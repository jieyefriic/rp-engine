//! xAI (Grok) provider implementation
//!
//! xAI API is compatible with OpenAI API format.
//! API: https://api.x.ai/v1/chat/completions

use async_trait::async_trait;

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse, StreamCallback,
    OpenAICompatibleProvider,
};

const XAI_API_URL: &str = "https://api.x.ai/v1/chat/completions";
const DEFAULT_MODEL: &str = "grok-3-mini";

/// xAI (Grok) provider
pub struct XAIProvider {
    inner: OpenAICompatibleProvider,
}

impl XAIProvider {
    /// Create a new xAI provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(
                api_key,
                XAI_API_URL,
                DEFAULT_MODEL,
                "xai",
            ),
        }
    }
}

#[async_trait]
impl LLMProvider for XAIProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, ProviderError> {
        self.inner.chat(messages, options).await
    }

    async fn stream(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
        on_chunk: StreamCallback,
    ) -> Result<StreamResponse, ProviderError> {
        self.inner.stream(messages, options, on_chunk).await
    }
}
