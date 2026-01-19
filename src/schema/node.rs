//! Node definitions for workflow graphs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::workflow::ToolRef;

/// Node types supported by the engine
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    /// Entry point node
    Start,
    /// LLM generation node
    Generate,
    /// Code execution node
    Execute,
    /// Dynamic task dispatcher
    Supervisor,
    /// External data source connector
    DataConnector,
    /// MCP tool connector
    Mcp,
    /// Final output node
    Response,
    /// User-defined custom node type
    #[serde(untagged)]
    Custom(String),
}

impl NodeType {
    /// Check if this node type can have input ports
    pub fn has_inputs(&self) -> bool {
        !matches!(self, NodeType::Start)
    }

    /// Check if this node type can have output ports
    pub fn has_outputs(&self) -> bool {
        !matches!(self, NodeType::Response)
    }
}

/// A node in the workflow graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YamlNode {
    /// Unique identifier for this node
    pub id: String,

    /// Type of node (determines executor)
    #[serde(rename = "type")]
    pub node_type: NodeType,

    /// Layer number for parallel execution
    /// Nodes in the same layer execute in parallel
    /// Layer 0 = start, higher layers execute after lower layers complete
    #[serde(default)]
    pub layer: u32,

    /// Node-specific configuration
    #[serde(default)]
    pub config: NodeConfig,
}

/// Configuration for different node types
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeConfig {
    // Common fields
    /// Output field mappings (for start node)
    #[serde(default)]
    pub output: HashMap<String, String>,

    // Generate node fields
    /// Template ID reference
    pub template: Option<String>,

    /// Provider ID (e.g., "openai", "anthropic")
    pub provider: Option<String>,

    /// Model ID (e.g., "gpt-4", "claude-sonnet-4")
    pub model: Option<String>,

    /// Variable bindings: var_name -> path expression
    #[serde(default)]
    pub variables: HashMap<String, String>,

    // Execute node fields
    /// Runtime for code execution (e.g., "python", "javascript")
    pub runtime: Option<String>,

    /// Code to execute (can be a path expression)
    pub code: Option<String>,

    // Data connector fields (legacy inline config)
    /// Source type (e.g., "rest_api", "postgresql")
    pub source: Option<String>,

    /// Connection configuration (legacy)
    #[serde(default)]
    pub connection: HashMap<String, serde_json::Value>,

    // Data connector fields (V2 with data source reference)
    /// Reference to a Data Source ID defined in workflow.data_sources
    pub data_source_id: Option<String>,

    /// Operation configuration (kind-specific)
    /// For rest_api: { path, method, headers, query_params, body, body_type }
    /// For postgresql: { operation, sql, table, columns, where, data, ... }
    /// For redis: { operation, key_template, value_template, ... }
    /// For minio: { operation, bucket, key, ... }
    /// For qdrant: { operation, collection, vector, ... }
    /// For graphql: { query, variables }
    #[serde(default)]
    pub operation: HashMap<String, serde_json::Value>,

    /// Parameter bindings for data connector templates
    /// Maps parameter name to state path (e.g., {"userId": "start.user_id"})
    #[serde(default)]
    pub bindings: HashMap<String, String>,

    // Supervisor fields
    /// Tools available to the Supervisor (ToolBox references)
    #[serde(default)]
    pub tools: Vec<ToolRef>,

    /// Maximum iterations for Supervisor loop
    pub max_iterations: Option<u32>,

    /// Maximum retry attempts per tool execution
    pub max_retry_per_tool: Option<u32>,

    /// Whether to execute tools in parallel (when dependencies allow)
    pub parallel: Option<bool>,

    /// Timeout for Supervisor execution in milliseconds
    pub timeout: Option<u64>,

    /// Maximum total result length (characters)
    pub max_result_length: Option<u64>,

    /// Number of tool failures before exiting with tool_failures reason
    pub fail_on_tool_count: Option<u32>,

    /// Template ID for summarizer (optional post-processing)
    pub summarizer_template: Option<String>,

    // MCP fields
    /// MCP server name
    pub server: Option<String>,

    /// MCP tool name
    pub tool: Option<String>,

    /// Tool arguments
    #[serde(default)]
    pub arguments: HashMap<String, String>,
}
