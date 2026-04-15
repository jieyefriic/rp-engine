//! Moonshot / Kimi (月之暗面) provider implementation
//!
//! Native Moonshot API — OpenAI-compatible format.
//! API: https://api.moonshot.cn/v1/chat/completions
//! Docs: https://platform.kimi.com/docs/api/chat

use async_trait::async_trait;

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse, StreamCallback,
    OpenAICompatibleProvider,
};

const MOONSHOT_API_URL: &str = "https://api.moonshot.cn/v1/chat/completions";
const DEFAULT_MODEL: &str = "kimi-k2.5";

/// Moonshot (Kimi) provider
pub struct MoonshotProvider {
    inner: OpenAICompatibleProvider,
}

impl MoonshotProvider {
    /// Create a new Moonshot provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(
                api_key,
                MOONSHOT_API_URL,
                DEFAULT_MODEL,
                "moonshot",
            ),
        }
    }
}

#[async_trait]
impl LLMProvider for MoonshotProvider {
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
