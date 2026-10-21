//! Atlas Cloud provider implementation
//!
//! Atlas Cloud exposes an OpenAI-compatible chat completions API.
//! API: https://api.atlascloud.ai/v1/chat/completions

use async_trait::async_trait;

use super::{
    ChatOptions, ChatResponse, LLMProvider, Message, OpenAICompatibleProvider, StreamCallback,
    StreamResponse,
};
use crate::error::ProviderError;

const ATLASCLOUD_API_URL: &str = "https://api.atlascloud.ai/v1/chat/completions";
const DEFAULT_MODEL: &str = "qwen/qwen3.5-flash";

/// Atlas Cloud provider
pub struct AtlasCloudProvider {
    inner: OpenAICompatibleProvider,
}

impl AtlasCloudProvider {
    /// Create a new Atlas Cloud provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAICompatibleProvider::new(
                api_key,
                ATLASCLOUD_API_URL,
                DEFAULT_MODEL,
                "atlascloud",
            ),
        }
    }
}

#[async_trait]
impl LLMProvider for AtlasCloudProvider {
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
