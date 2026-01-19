//! DeepSeek provider implementation
//!
//! DeepSeek API is compatible with OpenAI API format.
//! API: https://api.deepseek.com/chat/completions

use async_trait::async_trait;

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse, StreamCallback,
    OpenAICompatibleProvider,
};

const DEEPSEEK_API_URL: &str = "https://api.deepseek.com/chat/completions";
const DEFAULT_MODEL: &str = "deepseek-chat";

/// DeepSeek provider
pub struct DeepSeekProvider {
    inner: OpenAICompatibleProvider,
}

impl DeepSeekProvider {
    /// Create a new DeepSeek provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(
                api_key,
                DEEPSEEK_API_URL,
                DEFAULT_MODEL,
                "deepseek",
            ),
        }
    }
}

#[async_trait]
impl LLMProvider for DeepSeekProvider {
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
