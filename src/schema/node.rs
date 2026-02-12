//! Node definitions for workflow graphs

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

use super::workflow::ToolRef;

// ============================================================
// Template Configuration (supports conditional template selection)
// ============================================================

/// A single condition for template selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateCondition {
    /// Condition expression (e.g., "node.field == true")
    pub when: String,
    /// Template name to use when condition is satisfied
    #[serde(rename = "use")]
    pub use_template: String,
}

/// Template configuration - supports simple string or conditional selection
///
/// Simple usage (backward compatible):
/// ```yaml
/// template: my_template
/// ```
///
/// Conditional usage:
/// ```yaml
/// template:
///   default: normal_template
///   conditions:
///     - when: check.should_trigger == true
///       use: special_template
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TemplateSelector {
    /// Simple template name (backward compatible)
    Simple(String),
    /// Conditional template selection
    Conditional {
        /// Default template to use when no conditions match
        default: String,
        /// List of conditions evaluated in order; first match wins
        #[serde(default)]
        conditions: Vec<TemplateCondition>,
    },
}

impl TemplateSelector {
    /// Get the default/fallback template name
    pub fn default_template(&self) -> &str {
        match self {
            TemplateSelector::Simple(name) => name,
            TemplateSelector::Conditional { default, .. } => default,
        }
    }

    /// Get conditions (empty for simple config)
    pub fn conditions(&self) -> &[TemplateCondition] {
        match self {
            TemplateSelector::Simple(_) => &[],
            TemplateSelector::Conditional { conditions, .. } => conditions,
        }
    }
}

impl<'de> Deserialize<'de> for TemplateSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, Visitor, MapAccess};

        struct TemplateSelectorVisitor;

        impl<'de> Visitor<'de> for TemplateSelectorVisitor {
            type Value = TemplateSelector;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or object with 'default' and optional 'conditions'")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(TemplateSelector::Simple(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(TemplateSelector::Simple(value))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut default: Option<String> = None;
                let mut conditions: Vec<TemplateCondition> = Vec::new();

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "default" => {
                            default = Some(map.next_value()?);
                        }
                        "conditions" => {
                            conditions = map.next_value()?;
                        }
                        _ => {
                            // Ignore unknown fields
                            let _: serde_json::Value = map.next_value()?;
                        }
                    }
                }

                let default = default.ok_or_else(|| de::Error::missing_field("default"))?;
                Ok(TemplateSelector::Conditional { default, conditions })
            }
        }

        deserializer.deserialize_any(TemplateSelectorVisitor)
    }
}

// ============================================================
// Variable Binding
// ============================================================

/// Variable binding that supports single or multiple sources
/// - Single: `var_name: "path"` → one source (required)
/// - Multiple: `var_name: ["path1", "path2"]` → first available wins (required)
/// - Configured: `var_name: { from: "path", default: null }` → with default value (optional)
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum VariableBinding {
    /// Configured binding with default value: `{ from: "path" or ["path1", "path2"], default: value }`
    Configured {
        /// Source path(s) - can be single string or array
        from: StringOrArray,
        /// Default value when source is not available (makes binding optional)
        default: Option<serde_json::Value>,
    },
    /// Single source path: `"path"`
    Single(String),
    /// Multiple source paths (first available wins): `["path1", "path2"]`
    Multiple(Vec<String>),
}

/// Helper type for accepting either a single string or array of strings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrArray {
    Single(String),
    Multiple(Vec<String>),
}

impl StringOrArray {
    pub fn sources(&self) -> Vec<&str> {
        match self {
            StringOrArray::Single(s) => vec![s.as_str()],
            StringOrArray::Multiple(v) => v.iter().map(|s| s.as_str()).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            StringOrArray::Single(s) => s.is_empty(),
            StringOrArray::Multiple(v) => v.is_empty(),
        }
    }
}

impl VariableBinding {
    /// Get the first source path
    ///
    /// For single bindings, returns the path if non-empty.
    /// For multiple bindings, returns the first path.
    /// Most common use case - use this for simple single-source variables.
    pub fn first_source(&self) -> Option<&str> {
        match self {
            VariableBinding::Single(s) => if s.is_empty() { None } else { Some(s.as_str()) },
            VariableBinding::Multiple(v) => v.first().map(|s| s.as_str()),
            VariableBinding::Configured { from, .. } => from.sources().first().copied(),
        }
    }

    /// Get all source paths
    ///
    /// Use this when you need to iterate over multiple sources,
    /// e.g., for conditional inputs where any source having a value is sufficient.
    pub fn sources(&self) -> Vec<&str> {
        match self {
            VariableBinding::Single(s) => vec![s.as_str()],
            VariableBinding::Multiple(v) => v.iter().map(|s| s.as_str()).collect(),
            VariableBinding::Configured { from, .. } => from.sources(),
        }
    }

    /// Check if this binding is empty (no sources configured)
    pub fn is_empty(&self) -> bool {
        match self {
            VariableBinding::Single(s) => s.is_empty(),
            VariableBinding::Multiple(v) => v.is_empty(),
            VariableBinding::Configured { from, .. } => from.is_empty(),
        }
    }

    /// Check if this binding has a default value (making it optional)
    pub fn has_default(&self) -> bool {
        matches!(self, VariableBinding::Configured { default: Some(_), .. })
    }

    /// Get the default value if configured
    pub fn default_value(&self) -> Option<&serde_json::Value> {
        match self {
            VariableBinding::Configured { default, .. } => default.as_ref(),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for VariableBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, Visitor, MapAccess};

        struct VariableBindingVisitor;

        impl<'de> Visitor<'de> for VariableBindingVisitor {
            type Value = VariableBinding;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string, array of strings, or object with 'from' and optional 'default'")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(VariableBinding::Single(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(VariableBinding::Single(value))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<String>()? {
                    values.push(value);
                }
                Ok(VariableBinding::Multiple(values))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut from: Option<StringOrArray> = None;
                let mut default: Option<serde_json::Value> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "from" => {
                            from = Some(map.next_value()?);
                        }
                        "default" => {
                            default = Some(map.next_value()?);
                        }
                        _ => {
                            // Ignore unknown fields
                            let _: serde_json::Value = map.next_value()?;
                        }
                    }
                }

                let from = from.ok_or_else(|| de::Error::missing_field("from"))?;
                Ok(VariableBinding::Configured { from, default })
            }
        }

        deserializer.deserialize_any(VariableBindingVisitor)
    }
}

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
    /// Checkpoint node - pauses execution and returns state for later resume
    Checkpoint,
    /// Transform node - runs Rhai scripts for pure computation
    Transform,
    /// Web search node - queries search APIs (Tavily, Brave, Bocha, Perplexity)
    Search,
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
    /// Template configuration (simple string or conditional selection)
    pub template: Option<TemplateSelector>,

    /// Provider ID (e.g., "openai", "anthropic")
    pub provider: Option<String>,

    /// Model ID (e.g., "gpt-4", "claude-sonnet-4")
    pub model: Option<String>,

    /// Whether this generate node should stream its LLM response.
    /// When true and a StreamHandler is registered, uses provider.stream()
    /// instead of provider.chat(). Automatically disabled when output_schema
    /// is present (JSON mode requires complete response).
    #[serde(default)]
    pub stream: bool,

    /// Variable bindings: var_name -> path expression (supports array for conditional inputs)
    #[serde(default)]
    pub variables: HashMap<String, VariableBinding>,

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

    // Transform node fields
    /// Script file name (without .rhai extension), loaded from transforms_dir
    pub file: Option<String>,

    // Workflow node fields
    /// Sub-workflow reference (only used by workflow type nodes)
    /// References another workflow by name to execute as a sub-workflow
    #[serde(default)]
    pub workflow: Option<String>,
}
