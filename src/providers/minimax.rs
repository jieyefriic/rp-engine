//! MiniMax provider implementation
//!
//! MiniMax API is compatible with OpenAI API format.
//! API: https://api.minimax.io/v1/text/chatcompletion_v2

use async_trait::async_trait;

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse, StreamCallback,
    OpenAICompatibleProvider,
};

const MINIMAX_API_URL: &str = "https://api.minimax.io/v1/text/chatcompletion_v2";
const DEFAULT_MODEL: &str = "MiniMax-M2.5";

/// MiniMax provider
pub struct MiniMaxProvider {
    inner: OpenAICompatibleProvider,
}

impl MiniMaxProvider {
    /// Create a new MiniMax provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(
                api_key,
                MINIMAX_API_URL,
                DEFAULT_MODEL,
                "minimax",
            ),
        }
    }
}

#[async_trait]
impl LLMProvider for MiniMaxProvider {
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
