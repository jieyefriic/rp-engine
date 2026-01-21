//! Workflow definition - the top-level structure

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{YamlNode, YamlEdge, Template};
use crate::error::ValidationError;

/// Input field definition for workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputField {
    /// Field name
    pub name: String,

    /// Field type: string, number, boolean, object, array
    #[serde(rename = "type")]
    pub field_type: String,

    /// Whether this field is required
    #[serde(default)]
    pub required: bool,

    /// Default value if not provided
    pub default: Option<serde_json::Value>,
}

/// Input schema for the workflow
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputSchema {
    /// List of input fields
    #[serde(default)]
    pub fields: Vec<InputField>,
}

impl InputSchema {
    /// Validate input against schema
    pub fn validate(&self, input: &std::collections::HashMap<String, serde_json::Value>) -> Result<(), crate::error::EngineError> {
        for field in &self.fields {
            if field.required {
                match input.get(&field.name) {
                    None => {
                        return Err(crate::error::EngineError::MissingField(
                            format!("Required input field '{}' is missing", field.name)
                        ));
                    }
                    Some(value) if value.is_null() => {
                        return Err(crate::error::EngineError::MissingField(
                            format!("Required input field '{}' is null", field.name)
                        ));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// Data Source definition
/// Defines connection configuration for external data sources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSourceDef {
    /// Data source name (human-readable)
    pub name: String,

    /// Data source kind: rest_api, graphql, postgresql, redis, minio, qdrant
    pub kind: String,

    /// Optional description
    pub description: Option<String>,

    /// Connection configuration (kind-specific)
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Subgraph (ToolBox) definition
/// A mini-workflow that can be executed as a tool by Supervisor nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subgraph {
    /// Node definitions within the subgraph
    pub nodes: Vec<YamlNode>,

    /// Edge definitions within the subgraph
    pub edges: Vec<YamlEdge>,
}

/// Tool reference for Supervisor nodes
/// Maps a tool name to a subgraph that implements the tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRef {
    /// Tool name (used by Planner to call the tool)
    pub name: String,

    /// Human-readable description of what this tool does
    pub description: Option<String>,

    /// Reference to subgraph ID that implements this tool
    #[serde(rename = "ref")]
    pub subgraph_ref: String,
}

/// A fully prepared workflow ready for execution
///
/// This is the "fully prepared" YAML where:
/// - All node configs are complete
/// - Templates are attached (not just IDs)
/// - Variables have binding paths
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedWorkflow {
    /// Schema version
    #[serde(default = "default_version")]
    pub version: String,

    /// Workflow name
    pub name: String,

    /// Input schema
    #[serde(default)]
    pub input: InputSchema,

    /// Data Sources (data_source_id -> DataSourceDef)
    /// Defines connection configurations for data connector nodes
    #[serde(default)]
    pub data_sources: HashMap<String, DataSourceDef>,

    /// Node definitions
    pub nodes: Vec<YamlNode>,

    /// Edge definitions
    pub edges: Vec<YamlEdge>,

    /// Attached templates (template_id -> Template)
    #[serde(default)]
    pub templates: HashMap<String, Template>,

    /// Subgraphs (ToolBoxes) for Supervisor nodes
    /// Maps subgraph_id -> Subgraph definition
    #[serde(default)]
    pub subgraphs: HashMap<String, Subgraph>,
}

fn default_version() -> String {
    "0.1".to_string()
}

impl PreparedWorkflow {
    /// Parse a workflow from YAML string
    pub fn from_yaml(yaml: &str) -> Result<Self, ValidationError> {
        let workflow: PreparedWorkflow = serde_yaml::from_str(yaml)?;
        workflow.validate()?;
        Ok(workflow)
    }

    /// Validate the workflow structure
    pub fn validate(&self) -> Result<(), ValidationError> {
        // Check for start node
        let start_count = self.nodes.iter()
            .filter(|n| n.node_type == super::NodeType::Start)
            .count();

        if start_count == 0 {
            return Err(ValidationError::MissingField("start node".into()));
        }
        if start_count > 1 {
            return Err(ValidationError::InvalidNodeType("multiple start nodes".into()));
        }

        // Check for response node
        let response_count = self.nodes.iter()
            .filter(|n| n.node_type == super::NodeType::Response)
            .count();
        if response_count == 0 {
            return Err(ValidationError::MissingField("response node".into()));
        }

        // Check for duplicate node IDs
        let mut seen_ids = std::collections::HashSet::new();
        for node in &self.nodes {
            if !seen_ids.insert(&node.id) {
                return Err(ValidationError::DuplicateNodeId(node.id.clone()));
            }
        }

        // Validate edges reference valid nodes
        for edge in &self.edges {
            if !seen_ids.contains(&edge.from) {
                return Err(ValidationError::InvalidEdge {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                });
            }
            if !seen_ids.contains(&edge.to) {
                return Err(ValidationError::InvalidEdge {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                });
            }
        }

        // Validate layer order: from.layer < to.layer OR edge has max_iterations (loop)
        for edge in &self.edges {
            if let (Some(from_node), Some(to_node)) = (self.get_node(&edge.from), self.get_node(&edge.to)) {
                if from_node.layer >= to_node.layer && edge.max_iterations.is_none() {
                    return Err(ValidationError::InvalidEdge {
                        from: edge.from.clone(),
                        to: edge.to.clone(),
                    });
                }
            }
        }

        // Validate generate nodes have provider and model
        for node in &self.nodes {
            if node.node_type == super::NodeType::Generate {
                if node.config.provider.is_none() {
                    return Err(ValidationError::MissingField(
                        format!("provider for generate node '{}'", node.id)
                    ));
                }
                if node.config.model.is_none() {
                    return Err(ValidationError::MissingField(
                        format!("model for generate node '{}'", node.id)
                    ));
                }
            }
        }

        // Note: Template reference validation is done at Engine level,
        // since templates can come from workflow.templates or global templates

        Ok(())
    }

    /// Find the start node
    pub fn find_start_node(&self) -> Option<&YamlNode> {
        self.nodes.iter().find(|n| n.node_type == super::NodeType::Start)
    }

    /// Get a node by ID
    pub fn get_node(&self, id: &str) -> Option<&YamlNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Get outgoing edges for a node
    pub fn get_outgoing_edges(&self, node_id: &str) -> Vec<&YamlEdge> {
        self.edges.iter().filter(|e| e.from == node_id).collect()
    }

    /// Get incoming edges for a node
    pub fn get_incoming_edges(&self, node_id: &str) -> Vec<&YamlEdge> {
        self.edges.iter().filter(|e| e.to == node_id).collect()
    }

    /// Get a template by ID
    pub fn get_template(&self, id: &str) -> Option<&Template> {
        self.templates.get(id)
    }

    /// Get a subgraph by ID
    pub fn get_subgraph(&self, id: &str) -> Option<&Subgraph> {
        self.subgraphs.get(id)
    }

    /// Get a data source by ID
    pub fn get_data_source(&self, id: &str) -> Option<&DataSourceDef> {
        self.data_sources.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_workflow() {
        let yaml = r#"
version: "0.1"
name: "Simple Test"
nodes:
  - id: start
    type: start
    layer: 0
  - id: response
    type: response
    layer: 1
edges:
  - from: start
    to: response
"#;
        let workflow = PreparedWorkflow::from_yaml(yaml).unwrap();
        assert_eq!(workflow.name, "Simple Test");
        assert_eq!(workflow.nodes.len(), 2);
        assert_eq!(workflow.edges.len(), 1);
    }

    #[test]
    fn test_duplicate_node_id() {
        let yaml = r#"
version: "0.1"
name: "Test"
nodes:
  - id: start
    type: start
    layer: 0
  - id: start
    type: response
    layer: 1
edges: []
"#;
        let result = PreparedWorkflow::from_yaml(yaml);
        assert!(matches!(result, Err(ValidationError::DuplicateNodeId(_))));
    }

    #[test]
    fn test_missing_response_node() {
        let yaml = r#"
version: "0.1"
name: "Test"
nodes:
  - id: start
    type: start
    layer: 0
edges: []
"#;
        let result = PreparedWorkflow::from_yaml(yaml);
        assert!(matches!(result, Err(ValidationError::MissingField(_))));
    }

    #[test]
    fn test_invalid_layer_order() {
        let yaml = r#"
version: "0.1"
name: "Test"
nodes:
  - id: start
    type: start
    layer: 0
  - id: gen
    type: generate
    layer: 1
    config:
      provider: qwen
      model: qwen-turbo
  - id: response
    type: response
    layer: 2
edges:
  - from: start
    to: gen
  - from: gen
    to: start
  - from: gen
    to: response
"#;
        let result = PreparedWorkflow::from_yaml(yaml);
        // gen -> start is invalid because start.layer (0) < gen.layer (1)
        assert!(matches!(result, Err(ValidationError::InvalidEdge { .. })));
    }

    #[test]
    fn test_loop_with_max_iterations() {
        let yaml = r#"
version: "0.1"
name: "Test Loop"
nodes:
  - id: start
    type: start
    layer: 0
  - id: gen
    type: generate
    layer: 1
    config:
      provider: qwen
      model: qwen-turbo
  - id: response
    type: response
    layer: 2
edges:
  - from: start
    to: gen
  - from: gen
    to: gen
    max_iterations: 3
  - from: gen
    to: response
"#;
        // Loop edge with max_iterations is allowed
        let workflow = PreparedWorkflow::from_yaml(yaml).unwrap();
        assert_eq!(workflow.edges.len(), 3);
    }

    #[test]
    fn test_workflow_with_subgraphs() {
        let yaml = r#"
version: "0.1"
name: "Supervisor Test"
nodes:
  - id: start
    type: start
    layer: 0
  - id: supervisor
    type: supervisor
    layer: 1
    config:
      provider: qwen
      model: qwen-flash
      template: tpl_1
      max_iterations: 5
      tools:
        - name: search
          description: "Search tool"
          ref: toolbox_search
  - id: response
    type: response
    layer: 2
edges:
  - from: start
    to: supervisor
  - from: supervisor
    to: response
    exit_reason: default
subgraphs:
  toolbox_search:
    nodes:
      - id: gen
        type: generate
        layer: 1
        config:
          provider: qwen
          model: qwen-turbo
    edges: []
templates:
  tpl_1:
    user_prompt: "Plan: {{user_input}}"
"#;
        let workflow = PreparedWorkflow::from_yaml(yaml).unwrap();

        // Verify supervisor node
        let supervisor = workflow.get_node("supervisor").unwrap();
        assert_eq!(supervisor.config.max_iterations, Some(5));
        assert_eq!(supervisor.config.tools.len(), 1);
        assert_eq!(supervisor.config.tools[0].name, "search");

        // Verify subgraph
        assert!(workflow.subgraphs.contains_key("toolbox_search"));
        let subgraph = workflow.get_subgraph("toolbox_search").unwrap();
        assert_eq!(subgraph.nodes.len(), 1);

        // Verify exit edge
        let exit_edge = workflow.edges.iter()
            .find(|e| e.from == "supervisor")
            .unwrap();
        assert!(exit_edge.exit_reason.is_some());
    }
}
