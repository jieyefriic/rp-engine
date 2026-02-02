//! OpenAI provider implementation
//!
//! Supports:
//! - Standard models (GPT-4o, GPT-4-turbo, etc.)
//! - Reasoning models (GPT-5, o-series) with special handling:
//!   - Uses max_completion_tokens instead of max_tokens
//!   - Does not support temperature parameter
//!   - Supports reasoning_effort parameter (low/medium/high)
//!
//! API: https://api.openai.com/v1/chat/completions

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse,
    StreamCallback, TokenUsage, ReasoningEffort,
};
use super::content::build_openai_messages;
use super::streaming::{process_sse_stream, parse_openai_chunk};

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";
const DEFAULT_MODEL: &str = "gpt-4o-mini";

/// Models that require max_completion_tokens instead of max_tokens
/// AND don't support temperature parameter
const REASONING_MODELS: &[&str] = &[
    "gpt-5", "gpt-5-mini", "gpt-5-nano",
    "gpt-5.1", "gpt-5.1-codex", "gpt-5.1-codex-mini",
    "gpt-5.2", "gpt-5.2-chat-latest",
    "o4-mini", "o4", "o3", "o3-mini", "o1",
];

/// Check if a model is a reasoning model
fn is_reasoning_model(model: &str) -> bool {
    REASONING_MODELS.iter().any(|m| {
        model == *m || model.starts_with(&format!("{}-", m))
    })
}

/// OpenAI provider
pub struct OpenAIProvider {
    api_key: String,
    client: Client,
    base_url: String,
}

impl OpenAIProvider {
    /// Create a new OpenAI provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
            base_url: OPENAI_API_URL.to_string(),
        }
    }

    /// Set a custom base URL (for Azure OpenAI or proxies)
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Build request body for chat completion
    fn build_request_body(
        &self,
        messages: &[Message],
        options: &ChatOptions,
        stream: bool,
    ) -> Value {
        let model = options.model.as_deref().unwrap_or(DEFAULT_MODEL);
        let is_reasoning = is_reasoning_model(model);
        let max_tokens = options.max_tokens.unwrap_or(1024);

        let formatted_messages = build_openai_messages(messages, options.system_prompt.as_deref());

        let mut body = json!({
            "model": model,
            "messages": formatted_messages,
        });

        // Reasoning models don't support temperature
        if !is_reasoning {
            body["temperature"] = json!(options.temperature.unwrap_or(0.7));
        }

        // Reasoning models use max_completion_tokens, others use max_tokens
        if is_reasoning {
            body["max_completion_tokens"] = json!(max_tokens);

            // reasoning_effort for reasoning models
            if let Some(effort) = &options.reasoning_effort {
                body["reasoning_effort"] = json!(match effort {
                    ReasoningEffort::Low => "low",
                    ReasoningEffort::Medium => "medium",
                    ReasoningEffort::High => "high",
                });
            }
        } else {
            body["max_tokens"] = json!(max_tokens);
        }

        if stream {
            body["stream"] = json!(true);
        }

        if options.json_output {
            body["response_format"] = json!({"type": "json_object"});
        }

        body
    }

    /// Parse chat response from OpenAI JSON
    fn parse_response(&self, data: Value) -> Result<ChatResponse, ProviderError> {
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let model = data["model"]
            .as_str()
            .unwrap_or(DEFAULT_MODEL)
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
                // o-series reasoning tokens
                thinking_tokens: usage_obj.get("reasoning_tokens")
                    .and_then(|v| v.as_u64()),
                search_count: None,
            }
        } else {
            TokenUsage::default()
        };

        Ok(ChatResponse {
            content,
            thinking: None, // OpenAI doesn't return thinking content separately
            usage,
            model,
            raw: data,
        })
    }

    /// Handle API error response
    fn handle_error(&self, status: u16, text: &str) -> ProviderError {
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
impl LLMProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn default_model(&self) -> &str {
        DEFAULT_MODEL
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, ProviderError> {
        let body = self.build_request_body(&messages, &options, false);

        let response = self.client
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        let status = response.status().as_u16();
        let text = response.text().await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        if status >= 400 {
            return Err(self.handle_error(status, &text));
        }

        let data: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;

        self.parse_response(data)
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
    fn test_is_reasoning_model() {
        // Direct matches
        assert!(is_reasoning_model("gpt-5"));
        assert!(is_reasoning_model("gpt-5-mini"));
        assert!(is_reasoning_model("o4-mini"));
        assert!(is_reasoning_model("o3"));
        assert!(is_reasoning_model("o1"));

        // Prefix matches
        assert!(is_reasoning_model("gpt-5-preview"));
        assert!(is_reasoning_model("o3-mini-2024"));

        // Non-reasoning models
        assert!(!is_reasoning_model("gpt-4o"));
        assert!(!is_reasoning_model("gpt-4o-mini"));
        assert!(!is_reasoning_model("gpt-4-turbo"));
        assert!(!is_reasoning_model("gpt-3.5-turbo"));
    }

    #[test]
    fn test_build_request_body_standard_model() {
        let provider = OpenAIProvider::new("test-key");

        let messages = vec![Message::user("Hello")];
        let options = ChatOptions::new()
            .with_model("gpt-4o")
            .with_temperature(0.5)
            .with_max_tokens(100);

        let body = provider.build_request_body(&messages, &options, false);

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["max_tokens"], 100);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn test_build_request_body_reasoning_model() {
        let provider = OpenAIProvider::new("test-key");

        let messages = vec![Message::user("Hello")];
        let options = ChatOptions::new()
            .with_model("o3")
            .with_max_tokens(2000)
            .with_reasoning_effort(ReasoningEffort::High);

        let body = provider.build_request_body(&messages, &options, false);

        assert_eq!(body["model"], "o3");
        // Reasoning models don't have temperature
        assert!(body.get("temperature").is_none());
        // Uses max_completion_tokens instead of max_tokens
        assert_eq!(body["max_completion_tokens"], 2000);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn test_parse_response_with_reasoning_tokens() {
        let provider = OpenAIProvider::new("test-key");

        let data = json!({
            "choices": [{"message": {"content": "Answer"}}],
            "model": "o3",
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 50,
                "total_tokens": 60,
                "reasoning_tokens": 30
            }
        });

        let response = provider.parse_response(data).unwrap();

        assert_eq!(response.content, "Answer");
        assert_eq!(response.model, "o3");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 50);
        assert_eq!(response.usage.thinking_tokens, Some(30));
    }
}
