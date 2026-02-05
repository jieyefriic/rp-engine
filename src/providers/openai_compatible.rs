//! OpenAI-compatible provider implementation
//!
//! Base implementation for providers that use OpenAI-compatible API format:
//! DeepSeek, xAI, Qwen, Zhipu, Moonshot

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse,
    StreamCallback, TokenUsage,
};
use super::content::build_openai_messages;
use super::streaming::{process_sse_stream, parse_openai_chunk};

/// OpenAI-compatible provider for APIs that follow OpenAI's format
pub struct OpenAICompatibleProvider {
    api_key: String,
    client: Client,
    base_url: String,
    default_model: String,
    provider_name: String,
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAI-compatible provider
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        provider_name: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
            base_url: base_url.into(),
            default_model: default_model.into(),
            provider_name: provider_name.into(),
        }
    }

    /// Build request body for chat completion
    fn build_request_body(
        &self,
        messages: &[Message],
        options: &ChatOptions,
        stream: bool,
    ) -> Value {
        let model = options.model.as_deref().unwrap_or(&self.default_model);
        let temperature = options.temperature.unwrap_or(0.7);
        let max_tokens = options.max_tokens.unwrap_or(1024);

        let formatted_messages = build_openai_messages(messages, options.system_prompt.as_deref());

        let mut body = json!({
            "model": model,
            "messages": formatted_messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
        });

        if stream {
            body["stream"] = json!(true);
            body["stream_options"] = json!({"include_usage": true});
        }

        // DashScope/Qwen: enable web search
        // Note: enable_search is incompatible with response_format on DashScope,
        // so we skip json_output when search is enabled (the prompt still requests JSON)
        if options.enable_search && self.provider_name == "qwen" {
            body["enable_search"] = json!(true);
        } else if options.json_output {
            body["response_format"] = json!({"type": "json_object"});
        }

        body
    }

    /// Parse chat response from OpenAI-format JSON
    fn parse_response(&self, data: Value) -> Result<ChatResponse, ProviderError> {
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let model = data["model"]
            .as_str()
            .unwrap_or(&self.default_model)
            .to_string();

        let usage = if let Some(usage_obj) = data["usage"].as_object() {
            TokenUsage {
                input_tokens: usage_obj.get("prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: usage_obj.get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_tokens: usage_obj.get("total_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                thinking_tokens: None,
                search_count: None,
            }
        } else {
            TokenUsage::default()
        };

        Ok(ChatResponse {
            content,
            thinking: None,
            usage,
            model,
            raw: data,
        })
    }

    /// Handle API error response
    fn handle_error(&self, status: u16, text: &str) -> ProviderError {
        // Try to parse error message from response
        let message = serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| text.to_string());

        match status {
            401 => ProviderError::AuthenticationFailed,
            429 => ProviderError::RateLimited,
            _ => ProviderError::ApiError { status, message },
        }
    }
}

#[async_trait]
impl LLMProvider for OpenAICompatibleProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, ProviderError> {
        let model = options.model.as_deref().unwrap_or(&self.default_model);

        debug!(
            provider = %self.provider_name,
            model = %model,
            message_count = messages.len(),
            json_output = options.json_output,
            "Starting LLM chat request"
        );

        let body = self.build_request_body(&messages, &options, false);

        let response = self.client
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                error!(
                    provider = %self.provider_name,
                    model = %model,
                    url = %self.base_url,
                    error = %e,
                    "HTTP request failed"
                );
                ProviderError::RequestFailed(format!("{} request to {} failed: {}", self.provider_name, self.base_url, e))
            })?;

        let status = response.status().as_u16();
        let text = response.text().await
            .map_err(|e| {
                error!(
                    provider = %self.provider_name,
                    model = %model,
                    error = %e,
                    "Failed to read response body"
                );
                ProviderError::RequestFailed(format!("Failed to read response: {}", e))
            })?;

        if status >= 400 {
            error!(
                provider = %self.provider_name,
                model = %model,
                status = status,
                response_preview = %text.chars().take(500).collect::<String>(),
                "API returned error status"
            );
            return Err(self.handle_error(status, &text));
        }

        let data: Value = serde_json::from_str(&text)
            .map_err(|e| {
                error!(
                    provider = %self.provider_name,
                    model = %model,
                    error = %e,
                    response_preview = %text.chars().take(300).collect::<String>(),
                    "Failed to parse JSON response"
                );
                ProviderError::InvalidResponse(format!("JSON parse error: {} | Response: {}", e, text.chars().take(200).collect::<String>()))
            })?;

        let mut result = self.parse_response(data)?;

        // Track search call count for billing
        if options.enable_search && self.provider_name == "qwen" {
            result.usage.search_count = Some(1);
        }

        debug!(
            provider = %self.provider_name,
            model = %result.model,
            input_tokens = result.usage.input_tokens,
            output_tokens = result.usage.output_tokens,
            search_count = ?result.usage.search_count,
            response_len = result.content.len(),
            "LLM chat request completed"
        );

        Ok(result)
    }

    async fn stream(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
        on_chunk: StreamCallback,
    ) -> Result<StreamResponse, ProviderError> {
        let body = self.build_request_body(&messages, &options, true);

        let response = self.client
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        let status = response.status().as_u16();

        if status >= 400 {
            let text = response.text().await
                .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;
            return Err(self.handle_error(status, &text));
        }

        process_sse_stream(response, parse_openai_chunk, &on_chunk).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_body() {
        let provider = OpenAICompatibleProvider::new(
            "test-key",
            "https://api.example.com/chat",
            "test-model",
            "test-provider",
        );

        let messages = vec![Message::user("Hello")];
        let options = ChatOptions::new()
            .with_temperature(0.5)
            .with_max_tokens(100);

        let body = provider.build_request_body(&messages, &options, false);

        assert_eq!(body["model"], "test-model");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["max_tokens"], 100);
        assert!(body["messages"].is_array());
    }

    #[test]
    fn test_build_request_body_with_system_prompt() {
        let provider = OpenAICompatibleProvider::new(
            "key", "url", "model", "provider",
        );

        let messages = vec![Message::user("Hi")];
        let options = ChatOptions::new()
            .with_system_prompt("You are helpful");

        let body = provider.build_request_body(&messages, &options, false);
        let msgs = body["messages"].as_array().unwrap();

        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are helpful");
    }

    #[test]
    fn test_parse_response() {
        let provider = OpenAICompatibleProvider::new(
            "key", "url", "default-model", "provider",
        );

        let data = json!({
            "choices": [{"message": {"content": "Hello!"}}],
            "model": "gpt-4o",
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let response = provider.parse_response(data).unwrap();

        assert_eq!(response.content, "Hello!");
        assert_eq!(response.model, "gpt-4o");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.total_tokens, 15);
    }

    #[test]
    fn test_build_request_body_enable_search_qwen() {
        let provider = OpenAICompatibleProvider::new(
            "test-key",
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
            "qwen-plus",
            "qwen",
        );

        let messages = vec![Message::user("Hello")];
        let options = ChatOptions::new()
            .with_enable_search(true);

        let body = provider.build_request_body(&messages, &options, false);
        assert_eq!(body["enable_search"], true);
    }

    #[test]
    fn test_build_request_body_enable_search_not_added_for_non_qwen() {
        let provider = OpenAICompatibleProvider::new(
            "test-key",
            "https://api.deepseek.com/chat/completions",
            "deepseek-chat",
            "deepseek",
        );

        let messages = vec![Message::user("Hello")];
        let options = ChatOptions::new()
            .with_enable_search(true);

        let body = provider.build_request_body(&messages, &options, false);
        assert!(body.get("enable_search").is_none());
    }

    #[test]
    fn test_build_request_body_enable_search_default_false() {
        let provider = OpenAICompatibleProvider::new(
            "test-key",
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
            "qwen-plus",
            "qwen",
        );

        let messages = vec![Message::user("Hello")];
        let options = ChatOptions::new();

        let body = provider.build_request_body(&messages, &options, false);
        assert!(body.get("enable_search").is_none());
    }
}
