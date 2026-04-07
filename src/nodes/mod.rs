//! Node executors for different node types

mod start;
mod response;
mod checkpoint;
mod generate;
mod data_connector;
mod supervisor;
mod transform;
mod search;
mod connectors;
mod workflow_node;
mod iterator;

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::Value;

use crate::schema::{YamlNode, NodeType, NodeConfig, VariableBinding};
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;

// ============================================================================
// Shared Variable Resolution
// ============================================================================

/// Resolve all variable bindings against execution state.
///
/// For each binding source:
/// - If the source contains `.` → treat as state path (`node_id.field`) and read from state
/// - If the source does NOT contain `.` → treat as literal value, with smart type parsing:
///   - Integer strings (e.g. `"2"`) → `Value::Number`
///   - Float strings (e.g. `"3.14"`) → `Value::Number`
///   - `"true"` / `"false"` → `Value::Bool`
///   - Anything else → `Value::String`
/// - Falls back to `default` if configured, otherwise `Value::Null`
pub fn resolve_variables(
    variables: &HashMap<String, VariableBinding>,
    state: &ExecutionState,
) -> HashMap<String, Value> {
    let mut resolved = HashMap::new();

    for (name, binding) in variables {
        let value = binding.sources().iter()
            .filter(|path| !path.is_empty())
            .find_map(|path| {
                if path.contains('.') {
                    // State path reference (e.g. "start.user_id")
                    let v = state.read(path).unwrap_or(Value::Null);
                    if v.is_null() { None } else { Some(v) }
                } else {
                    // Literal value (e.g. "2", "true", "hello")
                    Some(parse_literal(path))
                }
            })
            .or_else(|| binding.default_value().cloned())
            .unwrap_or(Value::Null);

        resolved.insert(name.clone(), value);
    }

    resolved
}

/// Parse a literal string into a typed JSON value.
fn parse_literal(s: &str) -> Value {
    // Boolean
    if s == "true" { return Value::Bool(true); }
    if s == "false" { return Value::Bool(false); }
    // Integer (try before float to preserve integer type)
    if let Ok(n) = s.parse::<i64>() {
        return Value::Number(n.into());
    }
    // Float
    if let Ok(f) = s.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    // String fallback
    Value::String(s.to_string())
}

// ============================================================================
// Image Processing Utilities
// ============================================================================

use crate::providers::ContentPart;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use tracing::warn;

/// Resolve a single VariableBinding against state.
/// Returns the first non-null value from the binding's sources.
pub fn resolve_single_binding(
    binding: &VariableBinding,
    state: &ExecutionState,
) -> Option<Value> {
    binding.sources().iter()
        .filter(|path| !path.is_empty())
        .find_map(|path| {
            if path.contains('.') {
                let v = state.read(path).unwrap_or(Value::Null);
                if v.is_null() { None } else { Some(v) }
            } else {
                Some(parse_literal(path))
            }
        })
        .or_else(|| binding.default_value().cloned())
}

/// Parse an image input string into a ContentPart.
///
/// Supports three formats:
/// 1. Data URI: `data:image/jpeg;base64,/9j/4AAQ...` → ContentPart::Image
/// 2. HTTP URL: `https://example.com/photo.jpg` → ContentPart::ImageUrl
/// 3. Pure base64: `/9j/4AAQ...` → ContentPart::Image (media type detected via magic bytes)
pub fn parse_image_input(data: &str) -> Option<ContentPart> {
    if data.starts_with("data:") {
        // Data URI format: data:image/jpeg;base64,xxx
        let comma_pos = match data.find(',') {
            Some(pos) => pos,
            None => {
                warn!(data_preview = %data.chars().take(50).collect::<String>(), "Malformed data URI (no comma), skipping image");
                return None;
            }
        };
        let prefix = &data[..comma_pos];
        let base64_data = &data[comma_pos + 1..];

        let media_type = if let Some(semicolon) = prefix.find(';') {
            prefix[5..semicolon].to_string() // Skip "data:"
        } else {
            "image/jpeg".to_string()
        };

        Some(ContentPart::Image {
            media_type,
            data: base64_data.to_string(),
        })
    } else if data.starts_with("http://") || data.starts_with("https://") {
        // HTTP URL → ContentPart::ImageUrl
        Some(ContentPart::ImageUrl { url: data.to_string() })
    } else {
        // Pure base64 → detect media type from magic bytes
        let media_type = detect_image_media_type(data);
        Some(ContentPart::Image {
            media_type,
            data: data.to_string(),
        })
    }
}

/// Detect image media type from base64-encoded data using magic bytes.
///
/// Decodes the first 16 bytes and checks file signatures:
/// - JPEG: FF D8 FF
/// - PNG: 89 50 4E 47
/// - WebP: RIFF....WEBP
/// - GIF: GIF8
///
/// Returns "image/jpeg" as fallback if detection fails.
pub fn detect_image_media_type(base64_data: &str) -> String {
    // Only need the first ~20 base64 chars (decodes to ~15 bytes)
    let prefix = &base64_data[..base64_data.len().min(24)];
    match BASE64.decode(prefix) {
        Ok(bytes) if bytes.len() >= 4 => {
            match &bytes[0..4] {
                [0xFF, 0xD8, 0xFF, ..] => "image/jpeg".to_string(),
                [0x89, 0x50, 0x4E, 0x47] => "image/png".to_string(),
                [0x47, 0x49, 0x46, 0x38] => "image/gif".to_string(),
                [0x52, 0x49, 0x46, 0x46] if bytes.len() >= 12 && &bytes[8..12] == b"WEBP" => {
                    "image/webp".to_string()
                }
                _ => "image/jpeg".to_string(),
            }
        }
        _ => "image/jpeg".to_string(),
    }
}

pub use start::StartExecutor;
pub use response::ResponseExecutor;
pub use checkpoint::CheckpointExecutor;
pub use generate::GenerateExecutor;
pub use data_connector::DataConnectorExecutor;
pub use supervisor::SupervisorExecutor;
pub use transform::TransformExecutor;
pub use search::SearchExecutor;
pub use workflow_node::WorkflowNode;

// ============================================================================
// Custom Node Support
// ============================================================================

/// Trait for user-defined custom nodes
///
/// Implement this trait to create custom node types like STT, TTS, etc.
///
/// # Example
/// ```ignore
/// struct SttNode {
///     client: SttClient,
/// }
///
/// #[async_trait]
/// impl CustomNode for SttNode {
///     fn node_type(&self) -> &str {
///         "stt"
///     }
///
///     async fn execute(
///         &self,
///         config: &NodeConfig,
///         state: &ExecutionState,
///         engine_config: &ExecutionConfig,
///     ) -> Result<HashMap<String, Value>, EngineError> {
///         // Process audio and return transcription
///         let audio_path = config.variables.get("audio")...;
///         let text = self.client.transcribe(audio_path).await?;
///         Ok(HashMap::from([("text".to_string(), Value::String(text))]))
///     }
/// }
/// ```
#[async_trait]
pub trait CustomNode: Send + Sync {
    /// Returns the node type identifier (e.g., "stt", "tts")
    /// This must match the type used in YAML workflow definitions
    fn node_type(&self) -> &str;

    /// Execute the custom node logic
    async fn execute(
        &self,
        config: &NodeConfig,
        state: &ExecutionState,
        engine_config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError>;
}

/// Registry for custom node implementations
///
/// The registry maps node type names to their implementations.
/// Register custom nodes during engine initialization.
#[derive(Default)]
pub struct NodeRegistry {
    custom_nodes: HashMap<String, Arc<dyn CustomNode>>,
}

impl NodeRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            custom_nodes: HashMap::new(),
        }
    }

    /// Register a custom node implementation
    pub fn register(&mut self, node: Arc<dyn CustomNode>) {
        self.custom_nodes.insert(node.node_type().to_string(), node);
    }

    /// Get a custom node by type name
    pub fn get(&self, node_type: &str) -> Option<Arc<dyn CustomNode>> {
        self.custom_nodes.get(node_type).cloned()
    }

    /// Check if a custom node type is registered
    pub fn contains(&self, node_type: &str) -> bool {
        self.custom_nodes.contains_key(node_type)
    }

    /// Get all registered node type names
    pub fn node_types(&self) -> Vec<&str> {
        self.custom_nodes.keys().map(|s| s.as_str()).collect()
    }
}

// ============================================================================
// Built-in Node Executors
// ============================================================================

/// Trait for node execution
#[async_trait]
pub trait NodeExecutor: Send + Sync {
    /// Execute the node and return outputs
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError>;
}

/// Create an executor for a built-in node type
///
/// For custom node types, use `NodeRegistry::get()` instead.
pub fn create_executor(node_type: &NodeType) -> Box<dyn NodeExecutor> {
    match node_type {
        NodeType::Start => Box::new(StartExecutor),
        NodeType::Response => Box::new(ResponseExecutor),
        NodeType::Checkpoint => Box::new(CheckpointExecutor),
        NodeType::Generate => Box::new(GenerateExecutor),
        NodeType::DataConnector => Box::new(DataConnectorExecutor),
        NodeType::Supervisor => Box::new(SupervisorExecutor),
        NodeType::Transform => Box::new(TransformExecutor),
        NodeType::Search => Box::new(SearchExecutor),
        NodeType::Iterator => Box::new(iterator::IteratorExecutor),
        NodeType::Execute => Box::new(NotImplementedExecutor("execute")),
        NodeType::Mcp => Box::new(NotImplementedExecutor("mcp")),
        NodeType::Custom(type_name) => Box::new(CustomNodeNotFoundExecutor(type_name.clone())),
    }
}

/// Placeholder executor when a custom node type is not found in the registry
struct CustomNodeNotFoundExecutor(String);

#[async_trait]
impl NodeExecutor for CustomNodeNotFoundExecutor {
    async fn execute(
        &self,
        _node: &YamlNode,
        _state: &ExecutionState,
        _config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        Err(EngineError::NodeExecutionFailed(
            format!("Custom node type '{}' not found in registry. Did you forget to register it?", self.0)
        ))
    }
}

/// Placeholder executor for unimplemented node types
struct NotImplementedExecutor(&'static str);

#[async_trait]
impl NodeExecutor for NotImplementedExecutor {
    async fn execute(
        &self,
        _node: &YamlNode,
        _state: &ExecutionState,
        _config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        Err(EngineError::NodeExecutionFailed(
            format!("{} node type not yet implemented", self.0)
        ))
    }
}
