//! Zhipu (智谱) GLM provider implementation
//!
//! Native Zhipu API — OpenAI-compatible format.
//! API: https://open.bigmodel.cn/api/paas/v4/chat/completions
//! Docs: https://docs.bigmodel.cn/

use async_trait::async_trait;

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse, StreamCallback,
    OpenAICompatibleProvider,
};

const ZHIPU_API_URL: &str = "https://open.bigmodel.cn/api/paas/v4/chat/completions";
const DEFAULT_MODEL: &str = "glm-4.7";

/// Zhipu (智谱) GLM provider
pub struct ZhipuProvider {
    inner: OpenAICompatibleProvider,
}

impl ZhipuProvider {
    /// Create a new Zhipu provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(
                api_key,
                ZHIPU_API_URL,
                DEFAULT_MODEL,
                "zhipu",
            ),
        }
    }
}

#[async_trait]
impl LLMProvider for ZhipuProvider {
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
