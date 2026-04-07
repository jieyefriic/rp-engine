//! LLM Provider implementations
//!
//! Supports 9 LLM providers: OpenAI, Anthropic, Gemini, DeepSeek, xAI, Qwen, Zhipu, Moonshot, MiniMax
//! All LLM providers implement the LLMProvider trait with chat() and stream() methods.
//!
//! Also supports Embedding providers: DashScope, OpenAI
//! Embedding providers implement the EmbeddingProvider trait with embed() method.

mod content;
mod streaming;
mod openai;
mod anthropic;
mod gemini;
mod openai_compatible;
mod deepseek;
mod xai;
mod qwen;
mod zhipu;
mod moonshot;
mod minimax;
mod huoshan;
pub mod model_registry;

// Embedding providers
pub mod embedding;
mod embedding_dashscope;
mod embedding_openai;

// Search providers
pub mod search;

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

// Re-export LLM providers
pub use openai::OpenAIProvider;
pub use anthropic::AnthropicProvider;
pub use gemini::GeminiProvider;
pub use openai_compatible::OpenAICompatibleProvider;
pub use deepseek::DeepSeekProvider;
pub use xai::XAIProvider;
pub use qwen::QwenProvider;
pub use zhipu::ZhipuProvider;
pub use moonshot::MoonshotProvider;
pub use minimax::MiniMaxProvider;
pub use huoshan::HuoshanProvider;

// Re-export Embedding providers and types
pub use embedding::{EmbeddingProvider, EmbeddingRegistry, EmbeddingOptions, EmbeddingResponse, Embedding, EmbeddingUsage};
pub use embedding_dashscope::DashScopeEmbeddingProvider;
pub use embedding_openai::OpenAIEmbeddingProvider;

// Re-export content formatting
pub use content::{format_openai_content, format_anthropic_content, format_gemini_parts};

// ============================================================================
// Message Types
// ============================================================================

/// Message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Content part for multimodal messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentPart {
    Text {
        text: String,
    },
    /// Base64-encoded image data (no data: prefix)
    Image {
        #[serde(rename = "mediaType")]
        media_type: String,
        data: String,
    },
    /// HTTP/HTTPS image URL (passed directly to providers that support it)
    #[serde(rename = "image_url")]
    ImageUrl {
        url: String,
    },
}

/// Image input for VL (Vision-Language) requests
///
/// Supports both base64-encoded images and HTTP/HTTPS URLs.
///
/// # Example
/// ```ignore
/// // Base64 image
/// let image = ImageInput::new("image/png", base64_data);
/// // URL image
/// let image = ImageInput::url("https://example.com/photo.jpg");
/// let messages = vec![
///     Message::user_with_image_input("What's in this image?", &image),
/// ];
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ImageInput {
    /// Base64-encoded image data
    Base64 {
        media_type: String,
        data: String,
    },
    /// HTTP/HTTPS image URL
    Url {
        url: String,
    },
}

impl ImageInput {
    /// Create a new image input from base64 data
    pub fn new(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self::Base64 {
            media_type: media_type.into(),
            data: data.into(),
        }
    }

    /// Create an image input from a URL
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url { url: url.into() }
    }

    /// Create an image input for PNG
    pub fn png(data: impl Into<String>) -> Self {
        Self::new("image/png", data)
    }

    /// Create an image input for JPEG
    pub fn jpeg(data: impl Into<String>) -> Self {
        Self::new("image/jpeg", data)
    }

    /// Create an image input for WebP
    pub fn webp(data: impl Into<String>) -> Self {
        Self::new("image/webp", data)
    }

    /// Convert to a ContentPart
    pub fn to_content_part(&self) -> ContentPart {
        match self {
            ImageInput::Base64 { media_type, data } => ContentPart::Image {
                media_type: media_type.clone(),
                data: data.clone(),
            },
            ImageInput::Url { url } => ContentPart::ImageUrl {
                url: url.clone(),
            },
        }
    }
}

/// Message content - either plain text or multimodal parts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        MessageContent::Text(s)
    }
}

impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        MessageContent::Text(s.to_string())
    }
}

/// Chat message with multimodal support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: MessageContent,
    /// Tool call ID (for Tool role messages — identifies which tool call this responds to)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool calls requested by assistant (for Assistant role messages with function calling)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl Message {
    pub fn system(content: impl Into<MessageContent>) -> Self {
        Self { role: MessageRole::System, content: content.into(), tool_call_id: None, tool_calls: None }
    }

    pub fn user(content: impl Into<MessageContent>) -> Self {
        Self { role: MessageRole::User, content: content.into(), tool_call_id: None, tool_calls: None }
    }

    pub fn assistant(content: impl Into<MessageContent>) -> Self {
        Self { role: MessageRole::Assistant, content: content.into(), tool_call_id: None, tool_calls: None }
    }

    /// Create a tool result message
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: MessageContent::Text(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }

    /// Create a user message with a base64 image
    pub fn user_with_image(text: &str, media_type: &str, data: &str) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::Parts(vec![
                ContentPart::Text { text: text.to_string() },
                ContentPart::Image {
                    media_type: media_type.to_string(),
                    data: data.to_string(),
                },
            ]),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create a user message with an image URL
    pub fn user_with_image_url(text: &str, url: &str) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::Parts(vec![
                ContentPart::Text { text: text.to_string() },
                ContentPart::ImageUrl { url: url.to_string() },
            ]),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create a user message with an ImageInput
    pub fn user_with_image_input(text: &str, image: &ImageInput) -> Self {
        let mut parts = vec![ContentPart::Text { text: text.to_string() }];
        parts.push(image.to_content_part());
        Self {
            role: MessageRole::User,
            content: MessageContent::Parts(parts),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create a user message with multiple images
    pub fn user_with_images(text: &str, images: &[ImageInput]) -> Self {
        let mut parts = vec![ContentPart::Text { text: text.to_string() }];
        for image in images {
            parts.push(image.to_content_part());
        }
        Self {
            role: MessageRole::User,
            content: MessageContent::Parts(parts),
            tool_call_id: None,
            tool_calls: None,
        }
    }
}

// ============================================================================
// Chat Options
// ============================================================================

/// Reasoning effort level for OpenAI o-series models
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

// ============================================================================
// Tool Calling Types
// ============================================================================

/// Tool definition for native LLM function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name (must match exactly when LLM calls it)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for tool parameters
    pub parameters: serde_json::Value,
}

/// A tool call requested by the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool call (provider-assigned)
    pub id: String,
    /// Tool name to invoke
    pub name: String,
    /// Arguments as JSON
    pub arguments: serde_json::Value,
}

/// Chat request options
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    /// Model to use (provider default if None)
    pub model: Option<String>,
    /// Temperature (0.0-2.0, default 0.7)
    pub temperature: Option<f32>,
    /// Maximum tokens to generate (default 1024)
    pub max_tokens: Option<u32>,
    /// Top-p (nucleus) sampling
    pub top_p: Option<f32>,
    /// Top-k sampling
    pub top_k: Option<u32>,
    /// System prompt (separate from messages)
    pub system_prompt: Option<String>,
    /// Enable JSON output mode
    pub json_output: bool,

    // Provider-specific options
    /// OpenAI: reasoning effort for o-series models (low/medium/high)
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Anthropic/Gemini: extended thinking budget in tokens
    pub thinking_budget: Option<u32>,
    /// Anthropic: JSON schema for structured output
    pub output_schema: Option<serde_json::Value>,
    /// DashScope/Qwen: enable web search before responding
    pub enable_search: bool,
    /// Tool definitions for native function calling
    pub tools: Option<Vec<ToolDefinition>>,
}

impl ChatOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    pub fn with_top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_json_output(mut self) -> Self {
        self.json_output = true;
        self
    }

    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    pub fn with_thinking_budget(mut self, budget: u32) -> Self {
        self.thinking_budget = Some(budget);
        self
    }

    pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    pub fn with_enable_search(mut self, enable: bool) -> Self {
        self.enable_search = enable;
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }
}

// ============================================================================
// Response Types
// ============================================================================

/// Token usage information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub thinking_tokens: Option<u64>,
    /// Number of web search calls (DashScope bills per search call)
    pub search_count: Option<u64>,
}

/// Chat response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub thinking: Option<String>,
    pub usage: TokenUsage,
    pub model: String,
    pub raw: serde_json::Value,
    /// Tool calls requested by the LLM (present when LLM wants to use tools)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Stream response (returned after streaming completes)
#[derive(Debug, Clone)]
pub struct StreamResponse {
    pub content: String,
    /// Time to first token in milliseconds
    pub ttft: Option<f64>,
    /// Token usage (extracted from the final SSE chunk, if available)
    pub usage: Option<TokenUsage>,
}

/// Callback for stream chunks
/// Arguments: (chunk: &str, full_content: &str)
pub type StreamCallback = Box<dyn Fn(&str, &str) + Send + Sync>;

// ============================================================================
// Provider Trait
// ============================================================================

/// LLM Provider trait - all providers implement this
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Get the provider name (e.g., "openai", "anthropic")
    fn name(&self) -> &str;

    /// Get the default model for this provider
    fn default_model(&self) -> &str;

    /// Make a chat completion request
    async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, ProviderError>;

    /// Stream a chat completion
    async fn stream(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
        on_chunk: StreamCallback,
    ) -> Result<StreamResponse, ProviderError>;
}

// ============================================================================
// Provider Registry
// ============================================================================

/// Provider registry for managing multiple providers
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn LLMProvider>>,
}

impl ProviderRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self { providers: HashMap::new() }
    }

    /// Register a provider
    pub fn register(&mut self, name: impl Into<String>, provider: Arc<dyn LLMProvider>) {
        self.providers.insert(name.into(), provider);
    }

    /// Get a provider by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn LLMProvider>> {
        self.providers.get(name).cloned()
    }

    /// Check if a provider exists
    pub fn has(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get all registered provider names
    pub fn names(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}

/// Helper to create a registry with all providers
pub fn create_registry_with_keys(
    openai_key: Option<String>,
    anthropic_key: Option<String>,
    gemini_key: Option<String>,
    deepseek_key: Option<String>,
    xai_key: Option<String>,
    qwen_key: Option<String>,
    zhipu_key: Option<String>,
    moonshot_key: Option<String>,
    minimax_key: Option<String>,
    huoshan_key: Option<String>,
) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    if let Some(key) = openai_key {
        registry.register("openai", Arc::new(OpenAIProvider::new(key)));
    }

    if let Some(key) = anthropic_key {
        registry.register("anthropic", Arc::new(AnthropicProvider::new(key)));
    }

    if let Some(key) = gemini_key {
        registry.register("gemini", Arc::new(GeminiProvider::new(key)));
    }

    if let Some(key) = deepseek_key {
        registry.register("deepseek", Arc::new(DeepSeekProvider::new(key)));
    }

    if let Some(key) = xai_key {
        registry.register("xai", Arc::new(XAIProvider::new(key)));
    }

    if let Some(key) = qwen_key {
        registry.register("qwen", Arc::new(QwenProvider::new(key)));
    }

    if let Some(key) = zhipu_key {
        registry.register("zhipu", Arc::new(ZhipuProvider::new(key)));
    }

    if let Some(key) = moonshot_key {
        registry.register("moonshot", Arc::new(MoonshotProvider::new(key)));
    }

    if let Some(key) = minimax_key {
        registry.register("minimax", Arc::new(MiniMaxProvider::new(key)));
    }

    if let Some(key) = huoshan_key {
        registry.register("huoshan", Arc::new(HuoshanProvider::new(key)));
    }

    registry
}

// ============================================================================
// Provider Factory (from YAML ProviderDef)
// ============================================================================

/// Create a provider instance from a YAML ProviderDef.
///
/// The `provider_id` is the map key from the YAML `providers` section.
/// When `def.kind` is `None`, `provider_id` is used as the kind.
pub fn create_provider_from_def(
    provider_id: &str,
    def: &crate::schema::ProviderDef,
) -> Result<Arc<dyn LLMProvider>, crate::error::EngineError> {
    let kind = def.kind.as_deref().unwrap_or(provider_id);

    match kind {
        "openai" => {
            let mut p = OpenAIProvider::new(&def.api_key);
            if let Some(url) = &def.base_url {
                p = p.with_base_url(url);
            }
            Ok(Arc::new(p))
        }
        "anthropic" => {
            let mut p = AnthropicProvider::new(&def.api_key);
            if let Some(url) = &def.base_url {
                p = p.with_base_url(url);
            }
            Ok(Arc::new(p))
        }
        "gemini" => Ok(Arc::new(GeminiProvider::new(&def.api_key))),
        "deepseek" => Ok(Arc::new(DeepSeekProvider::new(&def.api_key))),
        "xai" => Ok(Arc::new(XAIProvider::new(&def.api_key))),
        "qwen" => Ok(Arc::new(QwenProvider::new(&def.api_key))),
        "zhipu" => Ok(Arc::new(ZhipuProvider::new(&def.api_key))),
        "moonshot" => Ok(Arc::new(MoonshotProvider::new(&def.api_key))),
        "minimax" => Ok(Arc::new(MiniMaxProvider::new(&def.api_key))),
        "huoshan" => Ok(Arc::new(HuoshanProvider::new(&def.api_key))),
        "openai_compatible" => {
            let base_url = def.base_url.as_deref()
                .ok_or_else(|| crate::error::EngineError::InvalidWorkflow(
                    format!("Provider '{}': openai_compatible requires base_url", provider_id)
                ))?;
            let default_model = def.default_model.as_deref().unwrap_or("default");
            Ok(Arc::new(OpenAICompatibleProvider::new(
                &def.api_key, base_url, default_model, provider_id,
            )))
        }
        unknown => Err(crate::error::EngineError::InvalidWorkflow(
            format!("Unknown provider kind '{}' for provider '{}'", unknown, provider_id)
        )),
    }
}

// ============================================================================
// Legacy Compatibility (ChatRequest)
// ============================================================================

/// Legacy ChatRequest - use ChatOptions instead
#[deprecated(since = "0.2.0", note = "Use ChatOptions with chat() method instead")]
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub json_mode: bool,
    pub thinking_budget: Option<u32>,
}

#[allow(deprecated)]
impl ChatRequest {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            json_mode: false,
            thinking_budget: None,
        }
    }

    pub fn with_message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    pub fn with_json_mode(mut self) -> Self {
        self.json_mode = true;
        self
    }

    /// Convert to new ChatOptions format
    pub fn to_options(&self) -> (Vec<Message>, ChatOptions) {
        let options = ChatOptions {
            model: Some(self.model.clone()),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            top_p: None,
            top_k: None,
            system_prompt: None,
            json_output: self.json_mode,
            reasoning_effort: None,
            thinking_budget: self.thinking_budget,
            output_schema: None,
            enable_search: false,
            tools: None,
        };
        (self.messages.clone(), options)
    }
}

#[cfg(test)]
mod tests_provider_factory {
    use super::*;
    use crate::schema::ProviderDef;

    fn make_def(kind: Option<&str>, api_key: &str, base_url: Option<&str>, default_model: Option<&str>) -> ProviderDef {
        ProviderDef {
            kind: kind.map(|s| s.to_string()),
            api_key: api_key.to_string(),
            base_url: base_url.map(|s| s.to_string()),
            default_model: default_model.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_known_providers_create_ok() {
        let kinds = ["openai", "anthropic", "gemini", "deepseek", "xai", "qwen", "zhipu", "moonshot", "minimax", "huoshan"];
        for kind in kinds {
            let def = make_def(None, "test-key", None, None);
            let result = create_provider_from_def(kind, &def);
            assert!(result.is_ok(), "Failed to create provider for kind '{}'", kind);
            assert_eq!(result.unwrap().name(), kind);
        }
    }

    #[test]
    fn test_kind_overrides_provider_id() {
        // provider_id is "my_openai" but kind is "openai"
        let def = make_def(Some("openai"), "test-key", None, None);
        let provider = create_provider_from_def("my_openai", &def).unwrap();
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn test_openai_compatible_requires_base_url() {
        let def = make_def(Some("openai_compatible"), "test-key", None, None);
        let result = create_provider_from_def("my_llm", &def);
        assert!(result.is_err());
        let err = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(err.contains("base_url"), "Error should mention base_url: {}", err);
    }

    #[test]
    fn test_openai_compatible_with_base_url() {
        let def = make_def(
            Some("openai_compatible"),
            "test-key",
            Some("http://localhost:8000/v1/chat/completions"),
            Some("my-model"),
        );
        let result = create_provider_from_def("my_llm", &def);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "my_llm");
    }

    #[test]
    fn test_unknown_kind_returns_error() {
        let def = make_def(Some("unknown_provider"), "test-key", None, None);
        let result = create_provider_from_def("test", &def);
        assert!(result.is_err());
        let err = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(err.contains("unknown_provider"));
    }

    #[test]
    fn test_openai_with_base_url() {
        let def = make_def(None, "test-key", Some("https://proxy.example.com/v1/chat/completions"), None);
        let result = create_provider_from_def("openai", &def);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "openai");
    }
}
