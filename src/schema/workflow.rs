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

    /// Data source kind: rest_api, graphql, postgresql, redis, s3, qdrant
    pub kind: String,

    /// Optional description
    pub description: Option<String>,

    /// Connection configuration (kind-specific)
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Provider definition for workflow-level LLM provider configuration.
/// Allows workflows to declare their own provider instances with API keys,
/// custom base URLs, and other configuration.
///
/// YAML example:
/// ```yaml
/// providers:
///   openai:
///     api_key: "${OPENAI_API_KEY}"
///   my_local_llm:
///     kind: openai_compatible
///     api_key: "not-needed"
///     base_url: "http://localhost:8000/v1/chat/completions"
///     default_model: "llama-3-8b"
/// ```
#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderDef {
    /// Provider kind: "openai", "anthropic", "gemini", "deepseek", "xai",
    /// "qwen", "zhipu", "moonshot", "minimax", "huoshan", "openai_compatible".
    /// When omitted, defaults to the provider ID (the map key).
    pub kind: Option<String>,

    /// API key for the provider (supports ${ENV_VAR} expansion in YAML)
    pub api_key: String,

    /// Custom base URL (for proxies, Azure OpenAI, vLLM, Ollama, etc.)
    /// Only meaningful for: openai, anthropic, openai_compatible
    pub base_url: Option<String>,

    /// Default model override (primarily for openai_compatible)
    pub default_model: Option<String>,
}

// Custom Debug to mask api_key in logs
impl std::fmt::Debug for ProviderDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let masked = if self.api_key.len() > 4 {
            format!("***{}", &self.api_key[self.api_key.len()-4..])
        } else {
            "****".to_string()
        };
        f.debug_struct("ProviderDef")
            .field("kind", &self.kind)
            .field("api_key", &masked)
            .field("base_url", &self.base_url)
            .field("default_model", &self.default_model)
            .finish()
    }
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
    /// Tool name (used by Planner/LLM to call the tool)
    pub name: String,

    /// Human-readable description of what this tool does
    pub description: Option<String>,

    /// Reference to subgraph ID that implements this tool
    #[serde(rename = "ref")]
    pub subgraph_ref: String,

    /// JSON Schema for tool parameters (for native function calling)
    /// If absent, tool accepts freeform arguments
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

/// Supported YAML spec versions
const SUPPORTED_VERSIONS: &[&str] = &["0.1", "1.0"];

/// A fully prepared workflow ready for execution
///
/// This is the "fully prepared" YAML where:
/// - All node configs are complete
/// - Templates are attached (not just IDs)
/// - Variables have binding paths
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedWorkflow {
    /// Schema version ("0.1" for legacy, "1.0" for current)
    #[serde(default = "default_version")]
    pub version: String,

    /// Workflow name (unique identifier)
    pub name: String,

    /// Human-readable description
    #[serde(default)]
    pub description: Option<String>,

    /// Input schema
    #[serde(default)]
    pub input: InputSchema,

    /// Data Sources (data_source_id -> DataSourceDef)
    /// Defines connection configurations for data connector nodes.
    /// Workflow-level data sources override Engine-level ones with the same ID.
    #[serde(default)]
    pub data_sources: HashMap<String, DataSourceDef>,

    /// Provider definitions (provider_id -> ProviderDef)
    /// Allows workflows to declare LLM providers with API keys inline.
    /// Workflow-level providers override Engine-level ones with the same ID.
    /// Supports ${ENV_VAR} expansion in api_key and other string fields.
    #[serde(default)]
    pub providers: HashMap<String, ProviderDef>,

    /// Node definitions
    pub nodes: Vec<YamlNode>,

    /// Edge definitions
    pub edges: Vec<YamlEdge>,

    /// Attached templates (template_id -> Template)
    #[serde(default)]
    pub templates: HashMap<String, Template>,

    /// Transform scripts (transform_name -> Rhai source code)
    /// Allows YAML to embed transform scripts inline, like templates.
    /// Workflow-level transforms override Engine-level ones with the same name.
    #[serde(default)]
    pub transforms: HashMap<String, String>,

    /// Subgraphs (ToolBoxes) for Supervisor nodes
    /// Maps subgraph_id -> Subgraph definition
    #[serde(default)]
    pub subgraphs: HashMap<String, Subgraph>,

    /// Harness configuration (workflow-level instructions for all generate nodes)
    #[serde(default)]
    pub harness: Option<HarnessConfig>,

    /// MCP server definitions (server_id -> McpServerDef)
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerDef>,
}

/// Harness configuration — workflow-level instructions injected into every generate node's system_prompt.
/// Similar to Claude Code's CLAUDE.md: always-present project rules + optional persistent memory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessConfig {
    /// Inline instruction text (injected before template system_prompt)
    #[serde(default)]
    pub instructions: Option<String>,

    /// External instruction file paths (contents read and concatenated)
    #[serde(default)]
    pub files: Vec<String>,

    /// Path to persistent memory file. Read at start, updated after execution via writeback.
    #[serde(default)]
    pub memory_file: Option<String>,

    /// Post-execution writeback configuration.
    /// When set, a dedicated LLM call reviews the workflow output + current memory
    /// and produces updated memory content, written back to memory_file.
    #[serde(default)]
    pub writeback: Option<WritebackConfig>,
}

/// Configuration for harness post-execution memory writeback.
/// A dedicated LLM call reviews execution results and updates the memory file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WritebackConfig {
    /// LLM provider ID for the writeback call
    pub provider: String,

    /// Model ID for the writeback call (can be a cheap model)
    pub model: String,

    /// Custom prompt for the writeback LLM. If omitted, a sensible default is used.
    #[serde(default)]
    pub prompt: Option<String>,

    /// Maximum character limit for the memory file. Prevents unbounded growth.
    /// Default: 2200 (following Hermes Agent convention, ~800 tokens)
    #[serde(default)]
    pub memory_char_limit: Option<usize>,
}

/// MCP Server definition for connecting to MCP tool servers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDef {
    /// MCP server URL (Streamable HTTP endpoint, e.g. "https://mcp.example.com/mcp")
    pub url: String,

    /// Optional authentication
    #[serde(default)]
    pub auth: Option<McpAuth>,
}

/// Authentication configuration for MCP servers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAuth {
    /// Auth type: "bearer" or "api_key"
    #[serde(rename = "type")]
    pub auth_type: String,

    /// Token value (supports ${ENV_VAR} expansion in YAML)
    pub token: String,
}

fn default_version() -> String {
    "1.0".to_string()
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
        // Validate version
        if !SUPPORTED_VERSIONS.contains(&self.version.as_str()) {
            return Err(ValidationError::MissingField(
                format!("Unsupported workflow version '{}'. Supported: {:?}", self.version, SUPPORTED_VERSIONS)
            ));
        }

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

        // Note: Layer-based validation removed. Execution now uses input-based dependency checking.
        // Cycles are protected by max_iterations on edges.

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

    #[test]
    fn test_workflow_with_providers() {
        let yaml = r#"
version: "1.0"
name: "Provider Test"
providers:
  openai:
    api_key: "sk-test-key-123"
  my_local:
    kind: openai_compatible
    api_key: "local-key"
    base_url: "http://localhost:8000/v1/chat/completions"
    default_model: "llama-3-8b"
nodes:
  - id: start
    type: start
  - id: gen
    type: generate
    config:
      provider: my_local
      model: llama-3-8b
  - id: response
    type: response
edges:
  - from: start
    to: gen
  - from: gen
    to: response
"#;
        let workflow = PreparedWorkflow::from_yaml(yaml).unwrap();
        assert_eq!(workflow.providers.len(), 2);

        // openai: kind omitted, defaults to None
        let openai = workflow.providers.get("openai").unwrap();
        assert!(openai.kind.is_none());
        assert_eq!(openai.api_key, "sk-test-key-123");
        assert!(openai.base_url.is_none());

        // my_local: openai_compatible with base_url
        let local = workflow.providers.get("my_local").unwrap();
        assert_eq!(local.kind.as_deref(), Some("openai_compatible"));
        assert_eq!(local.api_key, "local-key");
        assert_eq!(local.base_url.as_deref(), Some("http://localhost:8000/v1/chat/completions"));
        assert_eq!(local.default_model.as_deref(), Some("llama-3-8b"));
    }

    #[test]
    fn test_workflow_without_providers_backward_compat() {
        let yaml = r#"
version: "0.1"
name: "No Providers"
nodes:
  - id: start
    type: start
  - id: response
    type: response
edges:
  - from: start
    to: response
"#;
        let workflow = PreparedWorkflow::from_yaml(yaml).unwrap();
        assert!(workflow.providers.is_empty());
    }

    #[test]
    fn test_provider_def_debug_masks_key() {
        let def = ProviderDef {
            kind: Some("openai".to_string()),
            api_key: "sk-super-secret-key-12345".to_string(),
            base_url: None,
            default_model: None,
        };
        let debug_str = format!("{:?}", def);
        // Full key should NOT appear
        assert!(!debug_str.contains("sk-super-secret-key-12345"));
        // Last 4 chars should appear
        assert!(debug_str.contains("2345"));
    }
}
