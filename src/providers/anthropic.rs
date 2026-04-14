//! Anthropic Claude provider implementation
//!
//! Supports:
//! - Extended thinking with thinking.budget_tokens
//! - Structured outputs (JSON mode) with output_format
//! - Beta headers for new features
//!
//! API: https://api.anthropic.com/v1/messages

use async_trait::async_trait;
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{json, Value};

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse,
    StreamCallback, TokenUsage,
};
use super::content::{build_anthropic_messages, extract_system_prompt};
use super::streaming::{process_sse_stream, parse_anthropic_chunk};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Beta feature headers
const BETA_THINKING: &str = "interleaved-thinking-2025-05-14";
const BETA_STRUCTURED_OUTPUTS: &str = "structured-outputs-2025-11-13";

/// Anthropic provider
pub struct AnthropicProvider {
    api_key: String,
    client: Client,
    base_url: String,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
            base_url: ANTHROPIC_API_URL.to_string(),
        }
    }

    /// Set a custom base URL
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Build request headers
    fn build_headers(&self, options: &ChatOptions) -> HeaderMap {
        let mut headers = HeaderMap::new();

        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key).unwrap_or_else(|_| HeaderValue::from_static("invalid-key")),
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        headers.insert(
            "anthropic-dangerous-direct-browser-access",
            HeaderValue::from_static("true"),
        );
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/json"),
        );

        // Build beta headers list
        let mut beta_headers = Vec::new();

        // Extended thinking beta
        if options.thinking_budget.map(|b| b > 0).unwrap_or(false) {
            beta_headers.push(BETA_THINKING);
        }

        // Structured outputs beta
        if options.json_output {
            beta_headers.push(BETA_STRUCTURED_OUTPUTS);
        }

        if !beta_headers.is_empty() {
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_str(&beta_headers.join(",")).unwrap_or_else(|_| HeaderValue::from_static("")),
            );
        }

        headers
    }

    /// Build request body for chat completion
    fn build_request_body(
        &self,
        messages: &[Message],
        options: &ChatOptions,
        stream: bool,
    ) -> Value {
        let model = options.model.as_deref().unwrap_or(DEFAULT_MODEL);
        let temperature = options.temperature.unwrap_or(0.7);
        let max_tokens = options.max_tokens.unwrap_or(1024);

        // Build messages (excluding system - handled separately)
        let formatted_messages = build_anthropic_messages(messages);

        let mut body = json!({
            "model": model,
            "messages": formatted_messages,
            "max_tokens": max_tokens,
            "temperature": temperature,
        });

        if let Some(top_p) = options.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(top_k) = options.top_k {
            body["top_k"] = json!(top_k);
        }

        // System prompt (from options or extracted from messages)
        let system_prompt = options.system_prompt.clone()
            .or_else(|| extract_system_prompt(messages));
        if let Some(system) = system_prompt {
            body["system"] = json!(system);
        }

        // Extended thinking
        if let Some(budget) = options.thinking_budget {
            if budget > 0 {
                body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
            }
        }

        // JSON output mode with structured outputs
        if options.json_output {
            let schema = options.output_schema.clone().unwrap_or_else(|| {
                json!({
                    "type": "object",
                    "additionalProperties": true
                })
            });
            body["output_format"] = json!({
                "type": "json_schema",
                "schema": schema
            });
        }

        // Tool definitions (function calling)
        if let Some(ref tools) = options.tools {
            if !tools.is_empty() {
                let tool_defs: Vec<Value> = tools.iter().map(|t| {
                    json!({
                        "name": t.name,
                        "description": &t.description,
                        "input_schema": t.parameters.clone(),
                    })
                }).collect();
                body["tools"] = json!(tool_defs);
            }
        }

        if stream {
            body["stream"] = json!(true);
        }

        body
    }

    /// Parse chat response from Anthropic JSON
    fn parse_response(&self, data: Value) -> Result<ChatResponse, ProviderError> {
        // Anthropic returns content as an array of content blocks
        let content_blocks = data["content"].as_array();

        let content = content_blocks
            .map(|blocks| {
                blocks.iter()
                    .filter(|block| block["type"] == "text")
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let model = data["model"]
            .as_str()
            .unwrap_or(DEFAULT_MODEL)
            .to_string();

        let usage = if let Some(usage_obj) = data["usage"].as_object() {
            let input = usage_obj.get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = usage_obj.get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            TokenUsage {
                input_tokens: input,
                output_tokens: output,
                total_tokens: input + output,
                thinking_tokens: None, // Anthropic doesn't report this separately
                search_count: None,
            }
        } else {
            TokenUsage::default()
        };

        // Parse tool_use blocks from content
        let tool_calls = content_blocks
            .map(|blocks| {
                blocks.iter()
                    .filter(|block| block["type"] == "tool_use")
                    .filter_map(|block| {
                        let id = block["id"].as_str()?.to_string();
                        let name = block["name"].as_str()?.to_string();
                        let arguments = block["input"].clone();
                        Some(crate::providers::ToolCall { id, name, arguments })
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|v: &Vec<crate::providers::ToolCall>| !v.is_empty());

        Ok(ChatResponse {
            content,
            thinking: None,
            usage,
            model,
            raw: data,
            tool_calls,
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
impl LLMProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn default_model(&self) -> &str {
        DEFAULT_MODEL
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, ProviderError> {
        let headers = self.build_headers(&options);
        let body = self.build_request_body(&messages, &options, false);

        let response = self.client
            .post(&self.base_url)
            .headers(headers)
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
        let headers = self.build_headers(&options);
        let body = self.build_request_body(&messages, &options, true);

        let response = self.client
            .post(&self.base_url)
            .headers(headers)
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

        process_sse_stream(response, parse_anthropic_chunk, &on_chunk).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_headers_basic() {
        let provider = AnthropicProvider::new("test-key");
        let options = ChatOptions::new();

        let headers = provider.build_headers(&options);

        assert!(headers.contains_key("x-api-key"));
        assert!(headers.contains_key("anthropic-version"));
        assert!(!headers.contains_key("anthropic-beta"));
    }

    #[test]
    fn test_build_headers_with_thinking() {
        let provider = AnthropicProvider::new("test-key");
        let options = ChatOptions::new().with_thinking_budget(10000);

        let headers = provider.build_headers(&options);

        assert!(headers.contains_key("anthropic-beta"));
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert!(beta.contains(BETA_THINKING));
    }

    #[test]
    fn test_build_headers_with_json_output() {
        let provider = AnthropicProvider::new("test-key");
        let options = ChatOptions::new().with_json_output();

        let headers = provider.build_headers(&options);

        assert!(headers.contains_key("anthropic-beta"));
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert!(beta.contains(BETA_STRUCTURED_OUTPUTS));
    }

    #[test]
    fn test_build_request_body_basic() {
        let provider = AnthropicProvider::new("test-key");

        let messages = vec![Message::user("Hello")];
        let options = ChatOptions::new()
            .with_temperature(0.5)
            .with_max_tokens(100);

        let body = provider.build_request_body(&messages, &options, false);

        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["max_tokens"], 100);
    }

    #[test]
    fn test_build_request_body_with_thinking() {
        let provider = AnthropicProvider::new("test-key");

        let messages = vec![Message::user("Think about this")];
        let options = ChatOptions::new().with_thinking_budget(5000);

        let body = provider.build_request_body(&messages, &options, false);

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 5000);
    }

    #[test]
    fn test_build_request_body_with_json_output() {
        let provider = AnthropicProvider::new("test-key");

        let messages = vec![Message::user("Return JSON")];
        let options = ChatOptions::new().with_json_output();

        let body = provider.build_request_body(&messages, &options, false);

        assert_eq!(body["output_format"]["type"], "json_schema");
        assert!(body["output_format"]["schema"].is_object());
    }

    #[test]
    fn test_build_request_body_with_custom_schema() {
        let provider = AnthropicProvider::new("test-key");

        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        });

        let messages = vec![Message::user("Return JSON")];
        let options = ChatOptions::new()
            .with_json_output()
            .with_output_schema(schema.clone());

        let body = provider.build_request_body(&messages, &options, false);

        assert_eq!(body["output_format"]["schema"], schema);
    }

    #[test]
    fn test_parse_response() {
        let provider = AnthropicProvider::new("test-key");

        let data = json!({
            "content": [
                {"type": "text", "text": "Hello "},
                {"type": "text", "text": "World!"}
            ],
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });

        let response = provider.parse_response(data).unwrap();

        assert_eq!(response.content, "Hello World!");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.total_tokens, 15);
    }
}
