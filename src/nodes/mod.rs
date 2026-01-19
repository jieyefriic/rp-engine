//! Node executors for different node types

mod start;
mod response;
mod generate;
mod data_connector;
mod supervisor;
mod connectors;

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::Value;

use crate::schema::{YamlNode, NodeType, NodeConfig};
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;

pub use start::StartExecutor;
pub use response::ResponseExecutor;
pub use generate::GenerateExecutor;
pub use data_connector::DataConnectorExecutor;
pub use supervisor::SupervisorExecutor;

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
        NodeType::Generate => Box::new(GenerateExecutor),
        NodeType::DataConnector => Box::new(DataConnectorExecutor),
        NodeType::Supervisor => Box::new(SupervisorExecutor),
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
