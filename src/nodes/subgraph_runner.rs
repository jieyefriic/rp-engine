//! Shared subgraph execution utility
//!
//! Executes a subgraph's nodes in dependency order with isolated state.
//! Used by: Supervisor (tool execution), Iterator (per-item processing),
//! and Generate (tool calling).

use std::collections::HashMap;
use std::time::Instant;
use serde_json::{json, Value};
use tracing::debug;

use crate::schema::{YamlNode, YamlEdge, NodeType, Subgraph};
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::supervisor::{SupervisorTokenUsage, LLMCallUsage, SubgraphNodeExecution};

/// Result of executing a subgraph
pub struct SubgraphRunResult {
    /// Final outputs (from response node or last executed node)
    pub outputs: Value,
    /// Aggregated token usage across all nodes
    pub usage: SupervisorTokenUsage,
    /// Per-node execution details
    pub executions: Vec<SubgraphNodeExecution>,
}

/// Execute a subgraph with the given input in an isolated state.
///
/// Creates a fresh ExecutionState, runs nodes in dependency order,
/// and returns the final outputs with aggregated token usage.
pub async fn execute(
    subgraph: &Subgraph,
    input: HashMap<String, Value>,
    config: &ExecutionConfig,
) -> Result<SubgraphRunResult, EngineError> {
    // Create isolated execution state
    let mut state = ExecutionState::new(input);

    // Build node map and incoming edges
    let node_map: HashMap<&str, &YamlNode> = subgraph.nodes.iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    let incoming_edges: HashMap<&str, Vec<&YamlEdge>> = {
        let mut map: HashMap<&str, Vec<&YamlEdge>> = HashMap::new();
        for edge in &subgraph.edges {
            map.entry(edge.to.as_str()).or_default().push(edge);
        }
        map
    };

    // Check if node inputs are satisfied
    let inputs_satisfied = |node_id: &str, state: &ExecutionState| -> bool {
        let node = match node_map.get(node_id) {
            Some(n) => *n,
            None => return false,
        };

        if node.node_type == NodeType::Start {
            return true;
        }

        // Check variable bindings
        if !node.config.variables.is_empty() {
            for (_var_name, binding) in &node.config.variables {
                if binding.is_empty() || binding.has_default() {
                    continue;
                }
                let has_value = binding.sources().iter().any(|path| {
                    !path.is_empty() && state.has_output(path)
                });
                if !has_value {
                    return false;
                }
            }
            return true;
        }

        // Fall back to edge-based check
        let edges = incoming_edges.get(node_id).map(|v| v.as_slice()).unwrap_or(&[]);
        if edges.is_empty() {
            return true;
        }
        edges.iter().any(|e| state.has_node_output(&e.from))
    };

    // Initialize tracking
    let mut pending: std::collections::HashSet<String> = subgraph.nodes.iter()
        .map(|n| n.id.clone())
        .collect();
    let mut executed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut final_outputs: HashMap<String, Value> = HashMap::new();
    let mut total_usage = SupervisorTokenUsage::default();
    let mut executions: Vec<SubgraphNodeExecution> = Vec::new();

    // Execute nodes in dependency order
    while !pending.is_empty() {
        let ready: Vec<String> = pending.iter()
            .filter(|id| !executed.contains(*id) && inputs_satisfied(id, &state))
            .cloned()
            .collect();

        if ready.is_empty() {
            break;
        }

        for node_id in ready {
            let node = match node_map.get(node_id.as_str()) {
                Some(n) => *n,
                None => continue,
            };

            let executor = crate::nodes::create_executor(&node.node_type);
            let node_start = Instant::now();
            let outputs = executor.execute(node, &state, config).await?;
            let node_timing_ms = node_start.elapsed().as_millis() as u64;

            // Extract token usage
            let node_usage = if let Some(usage_val) = outputs.get("_usage") {
                let provider = usage_val.get("provider")
                    .and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                let model = usage_val.get("model")
                    .and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                let input_tokens = usage_val.get("input_tokens")
                    .and_then(|v| v.as_u64()).unwrap_or(0);
                let output_tokens = usage_val.get("output_tokens")
                    .and_then(|v| v.as_u64()).unwrap_or(0);
                LLMCallUsage::new(provider, model, input_tokens, output_tokens)
            } else {
                LLMCallUsage::default()
            };

            debug!(
                node = %node.id,
                timing_ms = node_timing_ms,
                input_tokens = node_usage.input_tokens,
                output_tokens = node_usage.output_tokens,
                "Subgraph node executed"
            );

            total_usage.add_from_llm_call(&node_usage);

            executions.push(SubgraphNodeExecution {
                node_id: node.id.clone(),
                usage: node_usage,
                timing_ms: node_timing_ms,
                outputs: json!(outputs.clone()),
            });

            state.write(&node.id, outputs.clone());

            if node.node_type == NodeType::Response {
                final_outputs = outputs;
            } else if final_outputs.is_empty() {
                final_outputs = outputs;
            }

            executed.insert(node_id.clone());
            pending.remove(&node_id);
        }
    }

    Ok(SubgraphRunResult {
        outputs: json!(final_outputs),
        usage: total_usage,
        executions,
    })
}
