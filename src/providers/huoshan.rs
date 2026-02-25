//! Huoshan (Doubao/豆包) provider implementation
//!
//! Huoshan/Doubao API is compatible with OpenAI API format.
//! API: https://ark.cn-beijing.volces.com/api/v3/chat/completions

use async_trait::async_trait;

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse, StreamCallback,
    OpenAICompatibleProvider,
};

const HUOSHAN_API_URL: &str = "https://ark.cn-beijing.volces.com/api/v3/chat/completions";
const DEFAULT_MODEL: &str = "doubao-seed-2-0-lite-260215";

/// Huoshan (Doubao/豆包) provider
pub struct HuoshanProvider {
    inner: OpenAICompatibleProvider,
}

impl HuoshanProvider {
    /// Create a new Huoshan provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(
                api_key,
                HUOSHAN_API_URL,
                DEFAULT_MODEL,
                "huoshan",
            ),
        }
    }
}

#[async_trait]
impl LLMProvider for HuoshanProvider {
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
