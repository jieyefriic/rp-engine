//! Graph traversal utilities

use std::collections::{HashMap, HashSet};

use crate::schema::{PreparedWorkflow, YamlNode, YamlEdge, NodeType};
use crate::state::ExecutionState;
use crate::error::EngineError;

/// Execution graph built from workflow definition
pub struct ExecutionGraph<'a> {
    /// Reference to the workflow
    workflow: &'a PreparedWorkflow,

    /// Node lookup by ID
    node_map: HashMap<&'a str, &'a YamlNode>,

    /// Outgoing edges by source node
    outgoing_edges: HashMap<&'a str, Vec<&'a YamlEdge>>,

    /// Incoming edges by target node
    incoming_edges: HashMap<&'a str, Vec<&'a YamlEdge>>,
}

impl<'a> ExecutionGraph<'a> {
    /// Build an execution graph from a workflow
    pub fn from_workflow(workflow: &'a PreparedWorkflow) -> Self {
        let mut node_map = HashMap::new();
        let mut outgoing_edges: HashMap<&str, Vec<&YamlEdge>> = HashMap::new();
        let mut incoming_edges: HashMap<&str, Vec<&YamlEdge>> = HashMap::new();

        // Build node map
        for node in &workflow.nodes {
            node_map.insert(node.id.as_str(), node);
        }

        // Build edge maps
        for edge in &workflow.edges {
            outgoing_edges
                .entry(edge.from.as_str())
                .or_default()
                .push(edge);
            incoming_edges
                .entry(edge.to.as_str())
                .or_default()
                .push(edge);
        }

        Self {
            workflow,
            node_map,
            outgoing_edges,
            incoming_edges,
        }
    }

    /// Find the start node
    pub fn find_start(&self) -> Result<String, EngineError> {
        self.workflow.nodes.iter()
            .find(|n| n.node_type == NodeType::Start)
            .map(|n| n.id.clone())
            .ok_or(EngineError::NoStartNode)
    }

    /// Get a node by ID
    pub fn get_node(&self, id: &str) -> Result<&YamlNode, EngineError> {
        self.node_map.get(id)
            .copied()
            .ok_or_else(|| EngineError::NodeNotFound(id.to_string()))
    }

    /// Get outgoing edges for a node
    pub fn get_outgoing_edges(&self, node_id: &str) -> Vec<&YamlEdge> {
        self.outgoing_edges
            .get(node_id)
            .map(|v| v.to_vec())
            .unwrap_or_default()
    }

    /// Get incoming edges for a node
    pub fn get_incoming_edges(&self, node_id: &str) -> Vec<&YamlEdge> {
        self.incoming_edges
            .get(node_id)
            .map(|v| v.to_vec())
            .unwrap_or_default()
    }

    /// Check if all dependencies for a node are satisfied (legacy edge-based)
    ///
    /// A node's dependencies are satisfied when all incoming edges
    /// have their source nodes executed.
    pub fn dependencies_met(&self, node_id: &str, executed: &HashSet<String>) -> bool {
        let incoming = self.get_incoming_edges(node_id);

        // Start node has no dependencies
        if incoming.is_empty() {
            return true;
        }

        // For non-start nodes, at least one incoming edge's source must be executed
        // This allows for conditional branches where only one path is taken
        incoming.iter().any(|edge| executed.contains(&edge.from))
    }

    /// Check if all input variables for a node are satisfied
    ///
    /// A node's inputs are satisfied when:
    /// 1. All paths in config.variables can be resolved from the current execution state
    /// 2. At least one incoming edge's condition is satisfied (if edges have conditions)
    ///
    /// This enables fine-grained dependency checking based on actual data flow
    /// AND respects conditional edges to prevent nodes from executing when their
    /// entry conditions are not met.
    pub fn inputs_satisfied(&self, node_id: &str, state: &ExecutionState) -> bool {
        let node = match self.get_node(node_id) {
            Ok(n) => n,
            Err(_) => return false,
        };

        // Start node always satisfied (no inputs)
        if node.node_type == NodeType::Start {
            return true;
        }

        // Check incoming edges - at least one must be "active"
        // An edge is active when:
        // 1. Its source node has been executed, AND
        // 2. Its condition (if any) evaluates to true
        let incoming = self.get_incoming_edges(node_id);
        if !incoming.is_empty() {
            let has_active_edge = incoming.iter().any(|edge| {
                // Source must have output
                if !state.has_node_output(&edge.from) {
                    return false;
                }

                // Check edge condition if present
                if let Some(condition) = &edge.condition {
                    match crate::expression::evaluate_expression(condition, state) {
                        Ok(result) => result,
                        Err(_) => false, // Condition evaluation failed, treat as false
                    }
                } else if edge.is_default {
                    // Default edges are always active when source is executed
                    true
                } else {
                    // No condition, not default -> unconditionally active
                    true
                }
            });

            if !has_active_edge {
                return false;
            }
        }

        // If no variables configured, edge check above is sufficient
        if node.config.variables.is_empty() {
            return true;
        }

        // Check each variable binding
        for (_var_name, binding) in &node.config.variables {
            // Skip empty bindings
            if binding.is_empty() {
                continue;
            }

            let sources = binding.sources();

            // Filter to only path references (contain ".") - literals like "2", "true" are always available
            let path_sources: Vec<_> = sources.iter()
                .filter(|path| !path.is_empty() && path.contains('.'))
                .collect();

            // If no path sources (all literals), this binding is satisfied
            if path_sources.is_empty() {
                continue;
            }

            // Check if any source has a value
            let has_value = path_sources.iter().any(|path| {
                state.has_output(path)
            });

            // If binding has a default value, it's optional and doesn't block execution
            if binding.has_default() {
                // Optional binding: don't block even if no value
                // The resolve logic will use the default value at runtime
                continue;
            }

            // Required binding (Single or Multiple without default): must have at least one value
            if !has_value {
                return false;
            }
        }

        true
    }

    /// Get all node IDs
    pub fn all_nodes(&self) -> Vec<&str> {
        self.node_map.keys().copied().collect()
    }

    /// Check if a node is a response node
    pub fn is_response_node(&self, node_id: &str) -> bool {
        self.node_map.get(node_id)
            .map(|n| n.node_type == NodeType::Response)
            .unwrap_or(false)
    }

    /// Check if a node is a supervisor node
    pub fn is_supervisor_node(&self, node_id: &str) -> bool {
        self.node_map.get(node_id)
            .map(|n| n.node_type == NodeType::Supervisor)
            .unwrap_or(false)
    }

    /// Get the workflow reference
    pub fn workflow(&self) -> &PreparedWorkflow {
        self.workflow
    }

    /// Perform topological sort to get execution order
    /// Returns None if there's a cycle
    pub fn topological_sort(&self) -> Option<Vec<String>> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut temp_visited = HashSet::new();

        for node_id in self.node_map.keys() {
            if !visited.contains(*node_id) {
                if !self.topo_visit(*node_id, &mut visited, &mut temp_visited, &mut result) {
                    return None; // Cycle detected
                }
            }
        }

        result.reverse();
        Some(result)
    }

    fn topo_visit(
        &self,
        node_id: &str,
        visited: &mut HashSet<String>,
        temp_visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) -> bool {
        if temp_visited.contains(node_id) {
            return false; // Cycle detected
        }
        if visited.contains(node_id) {
            return true;
        }

        temp_visited.insert(node_id.to_string());

        for edge in self.get_outgoing_edges(node_id) {
            if !self.topo_visit(&edge.to, visited, temp_visited, result) {
                return false;
            }
        }

        temp_visited.remove(node_id);
        visited.insert(node_id.to_string());
        result.push(node_id.to_string());

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_workflow() -> PreparedWorkflow {
        PreparedWorkflow::from_yaml(r#"
version: "0.1"
name: "Test"
nodes:
  - id: start
    type: start
  - id: gen
    type: generate
    config:
      provider: qwen
      model: qwen-flash
  - id: response
    type: response
edges:
  - from: start
    to: gen
  - from: gen
    to: response
"#).unwrap()
    }

    #[test]
    fn test_find_start() {
        let workflow = create_test_workflow();
        let graph = ExecutionGraph::from_workflow(&workflow);
        assert_eq!(graph.find_start().unwrap(), "start");
    }

    #[test]
    fn test_outgoing_edges() {
        let workflow = create_test_workflow();
        let graph = ExecutionGraph::from_workflow(&workflow);
        let edges = graph.get_outgoing_edges("start");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, "gen");
    }

    #[test]
    fn test_topological_sort() {
        let workflow = create_test_workflow();
        let graph = ExecutionGraph::from_workflow(&workflow);
        let order = graph.topological_sort().unwrap();
        assert_eq!(order, vec!["start", "gen", "response"]);
    }
}
