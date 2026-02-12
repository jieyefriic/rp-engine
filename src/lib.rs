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
//! use riceprompt_engine::{Engine, EngineBuilder};
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let engine = Engine::builder()
//!         .workflows_dir("./workflows")
//!         .prompts_dir("./prompts")
//!         .provider("qwen", std::env::var("QWEN_API_KEY")?)
//!         .build()
//!         .await?;
//!
//!     let result = engine.run("my_workflow", json!({
//!         "query": "Hello"
//!     })).await?;
//!
//!     println!("Output: {:?}", result.output);
//!     Ok(())
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
pub use engine::{ExecutionResult, ExecutionConfig, ExecutionStatus, Engine, EngineBuilder, StateSavingConfig, ConnectionPools, PoolConfig, ResumeParams, StreamHandler, SharedStreamHandler};

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

// Search provider exports
pub use providers::search::{
    SearchProvider, SearchProviderRegistry, SearchOptions, SearchResponse, SearchResult,
    TavilyProvider, BraveProvider, BochaProvider, PerplexitySearchProvider,
    create_search_registry,
};

// Legacy (deprecated) - kept for backward compatibility
#[allow(deprecated)]
pub use providers::ChatRequest;
