//! RicePrompt-Engine: YAML-native Agent workflow execution engine
//!
//! This crate provides a powerful engine for executing agent workflows
//! defined in YAML format. It supports:
//!
//! - Multiple node types: start, generate, data_connector, response, supervisor
//! - LLM provider integrations: OpenAI, Anthropic, Gemini, DeepSeek, etc.
//! - Expression language for edge conditions
//! - Append-only state with short ID references
//! - Loop protection with max_iterations
//!
//! # Example
//!
//! ```ignore
//! use riceprompt_engine::{execute, ExecutionConfig, ProviderRegistry};
//! use std::collections::HashMap;
//!
//! #[tokio::main]
//! async fn main() {
//!     let yaml = include_str!("workflow.yaml");
//!     let input = HashMap::from([("query".into(), "Hello".into())]);
//!     let config = ExecutionConfig::default();
//!
//!     let result = execute(yaml, input, config).await.unwrap();
//!     println!("Output: {:?}", result.output);
//! }
//! ```

pub mod error;
pub mod schema;
pub mod state;
pub mod expression;
pub mod engine;
pub mod nodes;
pub mod providers;

// Re-exports
pub use error::EngineError;
pub use schema::{PreparedWorkflow, YamlNode, YamlEdge, Template, NodeConfig, NodeType};
pub use state::{ExecutionState, ExecutionRecord, CheckpointState, EdgeCountEntry};
pub use engine::{execute, ExecutionResult, ExecutionConfig, ExecutionStatus, Engine, EngineBuilder, StateSavingConfig, ConnectionPools, PoolConfig, ResumeParams, StreamHandler, SharedStreamHandler};

// Custom node support
pub use nodes::{CustomNode, NodeRegistry};

// Provider exports
pub use providers::{
    // Core types
    LLMProvider, ProviderRegistry, Message, MessageRole, MessageContent, ContentPart,
    ChatOptions, ChatResponse, StreamResponse, StreamCallback, TokenUsage,
    ReasoningEffort, ImageInput,
    // Providers
    OpenAIProvider, AnthropicProvider, GeminiProvider,
    DeepSeekProvider, XAIProvider, QwenProvider, ZhipuProvider, MoonshotProvider,
    OpenAICompatibleProvider,
    // Registry helper
    create_registry_with_keys,
    // Embedding types
    EmbeddingProvider, EmbeddingRegistry, EmbeddingOptions, EmbeddingResponse, Embedding, EmbeddingUsage,
    DashScopeEmbeddingProvider, OpenAIEmbeddingProvider,
};

// Legacy (deprecated) - kept for backward compatibility
#[allow(deprecated)]
pub use providers::ChatRequest;
