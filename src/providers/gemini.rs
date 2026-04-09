//! Google Gemini provider implementation
//!
//! Supports:
//! - Thinking with thinkingConfig.thinkingBudget
//! - System instruction via systemInstruction field
//! - Response parts separation (thought=true vs output)
//!
//! API: https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::error::ProviderError;
use super::{
    LLMProvider, Message, ChatOptions, ChatResponse, StreamResponse,
    StreamCallback, TokenUsage,
};
use super::content::{build_gemini_contents, extract_system_prompt};
use super::streaming::{process_sse_stream, parse_gemini_chunk};

const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_MODEL: &str = "gemini-2.0-flash";

/// Gemini provider
pub struct GeminiProvider {
    api_key: String,
    client: Client,
}

impl GeminiProvider {
    /// Create a new Gemini provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    /// Build the API URL for a given model and operation
    fn build_url(&self, model: &str, stream: bool) -> String {
        let operation = if stream {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };
        format!("{}/{}:{}?key={}", GEMINI_API_BASE, model, operation, self.api_key)
    }

    /// Build request body for chat completion
    fn build_request_body(
        &self,
        messages: &[Message],
        options: &ChatOptions,
    ) -> Value {
        let temperature = options.temperature.unwrap_or(0.7);
        let max_tokens = options.max_tokens.unwrap_or(1024);

        // Build contents array (excluding system - handled separately)
        let contents = build_gemini_contents(messages);

        // Build generation config
        let mut generation_config = json!({
            "temperature": temperature,
            "maxOutputTokens": max_tokens,
        });

        if let Some(top_p) = options.top_p {
            generation_config["topP"] = json!(top_p);
        }
        if let Some(top_k) = options.top_k {
            generation_config["topK"] = json!(top_k);
        }

        // Thinking budget support
        // thinkingBudget: 0 = disable thinking, >0 = set budget, null = use model default
        if let Some(budget) = options.thinking_budget {
            generation_config["thinkingConfig"] = json!({
                "thinkingBudget": budget
            });
        }

        // JSON output mode
        if options.json_output {
            generation_config["responseMimeType"] = json!("application/json");
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": generation_config,
        });

        // System instruction (from options or extracted from messages)
        let system_prompt = options.system_prompt.clone()
            .or_else(|| extract_system_prompt(messages));
        if let Some(system) = system_prompt {
            body["systemInstruction"] = json!({
                "parts": [{ "text": system }]
            });
        }

        // Tool definitions (function calling)
        if let Some(ref tools) = options.tools {
            if !tools.is_empty() {
                let function_declarations: Vec<Value> = tools.iter().map(|t| {
                    let mut decl = json!({
                        "name": t.name,
                        "description": &t.description,
                    });
                    if !t.parameters.is_null() {
                        decl["parameters"] = t.parameters.clone();
                    }
                    decl
                }).collect();
                body["tools"] = json!([{
                    "functionDeclarations": function_declarations
                }]);
            }
        }

        body
    }

    /// Parse chat response from Gemini JSON
    fn parse_response(&self, data: Value) -> Result<ChatResponse, ProviderError> {
        // Extract parts from response
        let parts = data["candidates"][0]["content"]["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        // Separate thinking parts (thought=true) from output parts
        let mut content = String::new();
        let mut thinking = None;

        for part in &parts {
            let is_thought = part["thought"].as_bool().unwrap_or(false);
            if let Some(text) = part["text"].as_str() {
                if is_thought {
                    thinking = Some(text.to_string());
                } else {
                    content.push_str(text);
                }
            }
        }

        // Parse usage metadata
        let usage = if let Some(meta) = data["usageMetadata"].as_object() {
            TokenUsage {
                input_tokens: meta.get("promptTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: meta.get("candidatesTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_tokens: meta.get("totalTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                thinking_tokens: meta.get("thoughtsTokenCount")
                    .and_then(|v| v.as_u64()),
                search_count: None,
            }
        } else {
            TokenUsage::default()
        };

        // Parse functionCall parts as tool_calls
        let tool_calls: Vec<crate::providers::ToolCall> = parts.iter()
            .filter_map(|part| {
                let fc = part.get("functionCall")?;
                let name = fc["name"].as_str()?.to_string();
                let args = fc["args"].clone();
                // Gemini doesn't provide a call ID, generate one
                let id = format!("gemini_call_{}", name);
                Some(crate::providers::ToolCall { id, name, arguments: args })
            })
            .collect();

        Ok(ChatResponse {
            content,
            thinking,
            usage,
            model: DEFAULT_MODEL.to_string(),
            raw: data,
            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
        })
    }

    /// Handle API error response
    fn handle_error(&self, status: u16, text: &str) -> ProviderError {
        let message = serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| text.to_string());

        match status {
            401 | 403 => ProviderError::AuthenticationFailed,
            429 => ProviderError::RateLimited,
            _ => ProviderError::ApiError { status, message },
        }
    }
}

#[async_trait]
impl LLMProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn default_model(&self) -> &str {
        DEFAULT_MODEL
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> Result<ChatResponse, ProviderError> {
        let model = options.model.as_deref().unwrap_or(DEFAULT_MODEL);
        let url = self.build_url(model, false);
        let body = self.build_request_body(&messages, &options);

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
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
        let model = options.model.as_deref().unwrap_or(DEFAULT_MODEL);
        let url = self.build_url(model, true);
        let body = self.build_request_body(&messages, &options);

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
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

        process_sse_stream(response, parse_gemini_chunk, &on_chunk).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_url() {
        let provider = GeminiProvider::new("test-key");

        let url = provider.build_url("gemini-2.0-flash", false);
        assert!(url.contains("gemini-2.0-flash:generateContent"));
        assert!(url.contains("key=test-key"));

        let stream_url = provider.build_url("gemini-2.0-flash", true);
        assert!(stream_url.contains("streamGenerateContent?alt=sse"));
    }

    #[test]
    fn test_build_request_body_basic() {
        let provider = GeminiProvider::new("test-key");

        let messages = vec![Message::user("Hello")];
        let options = ChatOptions::new()
            .with_temperature(0.5)
            .with_max_tokens(100);

        let body = provider.build_request_body(&messages, &options);

        assert_eq!(body["generationConfig"]["temperature"], 0.5);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 100);
        assert!(body["contents"].is_array());
    }

    #[test]
    fn test_build_request_body_with_thinking() {
        let provider = GeminiProvider::new("test-key");

        let messages = vec![Message::user("Think hard")];
        let options = ChatOptions::new().with_thinking_budget(8192);

        let body = provider.build_request_body(&messages, &options);

        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingBudget"], 8192);
    }

    #[test]
    fn test_build_request_body_with_system_instruction() {
        let provider = GeminiProvider::new("test-key");

        let messages = vec![Message::user("Hi")];
        let options = ChatOptions::new()
            .with_system_prompt("You are a helpful assistant");

        let body = provider.build_request_body(&messages, &options);

        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "You are a helpful assistant"
        );
    }

    #[test]
    fn test_build_request_body_with_json_output() {
        let provider = GeminiProvider::new("test-key");

        let messages = vec![Message::user("Return JSON")];
        let options = ChatOptions::new().with_json_output();

        let body = provider.build_request_body(&messages, &options);

        assert_eq!(body["generationConfig"]["responseMimeType"], "application/json");
    }

    #[test]
    fn test_parse_response_basic() {
        let provider = GeminiProvider::new("test-key");

        let data = json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello World!"}]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "totalTokenCount": 15
            }
        });

        let response = provider.parse_response(data).unwrap();

        assert_eq!(response.content, "Hello World!");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.total_tokens, 15);
        assert!(response.thinking.is_none());
    }

    #[test]
    fn test_parse_response_with_thinking() {
        let provider = GeminiProvider::new("test-key");

        let data = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Let me think...", "thought": true},
                        {"text": "The answer is 42"}
                    ]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "totalTokenCount": 30,
                "thoughtsTokenCount": 15
            }
        });

        let response = provider.parse_response(data).unwrap();

        assert_eq!(response.content, "The answer is 42");
        assert_eq!(response.thinking, Some("Let me think...".to_string()));
        assert_eq!(response.usage.thinking_tokens, Some(15));
    }
}
