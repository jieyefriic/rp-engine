//! DashScope Embedding Provider
//!
//! API: https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings
//! Default model: text-embedding-v4
//! Dimensions: 64-2048 (default 1024)

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::error::ProviderError;
use super::embedding::{EmbeddingProvider, EmbeddingOptions, EmbeddingResponse, Embedding, EmbeddingUsage};

const API_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings";
const DEFAULT_MODEL: &str = "text-embedding-v4";

/// DashScope embedding provider
pub struct DashScopeEmbeddingProvider {
    api_key: String,
    client: Client,
}

impl DashScopeEmbeddingProvider {
    /// Create a new DashScope embedding provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    /// Build the request body for the embeddings API
    fn build_request_body(&self, texts: &[String], options: &EmbeddingOptions) -> Value {
        let model = options.model.as_deref().unwrap_or(DEFAULT_MODEL);

        let mut body = json!({
            "model": model,
            "input": texts,
            "encoding_format": "float"
        });

        // Add dimension if specified (text-embedding-v4 supports 64-2048)
        if let Some(dim) = options.dimension {
            body["dimensions"] = json!(dim);
        }

        body
    }

    /// Parse the API response
    fn parse_response(&self, response: Value) -> Result<EmbeddingResponse, ProviderError> {
        // Extract data array
        let data = response["data"]
            .as_array()
            .ok_or_else(|| ProviderError::InvalidResponse("Missing data array".to_string()))?;

        let embeddings: Vec<Embedding> = data
            .iter()
            .filter_map(|item| {
                let embedding = item["embedding"].as_array()?;
                let vector: Vec<f32> = embedding
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();
                let index = item["index"].as_u64().unwrap_or(0) as usize;
                Some(Embedding { vector, index })
            })
            .collect();

        if embeddings.is_empty() {
            return Err(ProviderError::InvalidResponse("No embeddings in response".to_string()));
        }

        // Extract usage
        let usage = EmbeddingUsage {
            prompt_tokens: response["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            total_tokens: response["usage"]["total_tokens"].as_u64().unwrap_or(0),
        };

        // Extract model
        let model = response["model"]
            .as_str()
            .unwrap_or(DEFAULT_MODEL)
            .to_string();

        Ok(EmbeddingResponse {
            embeddings,
            usage,
            model,
        })
    }

    /// Handle API error response
    fn handle_error(&self, status: reqwest::StatusCode, body: &str) -> ProviderError {
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return ProviderError::AuthenticationFailed;
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return ProviderError::RateLimited;
        }

        // Try to parse error message from response
        if let Ok(error_json) = serde_json::from_str::<Value>(body) {
            let message = error_json["error"]["message"]
                .as_str()
                .or_else(|| error_json["message"].as_str())
                .unwrap_or(body);
            return ProviderError::ApiError {
                status: status.as_u16(),
                message: message.to_string(),
            };
        }

        ProviderError::ApiError {
            status: status.as_u16(),
            message: body.to_string(),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for DashScopeEmbeddingProvider {
    fn name(&self) -> &str {
        "dashscope"
    }

    fn default_model(&self) -> &str {
        DEFAULT_MODEL
    }

    async fn embed(
        &self,
        texts: Vec<String>,
        options: EmbeddingOptions,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let body = self.build_request_body(&texts, &options);

        let response = self
            .client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        let status = response.status();
        let body_text = response
            .text()
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        if !status.is_success() {
            return Err(self.handle_error(status, &body_text));
        }

        let response_json: Value = serde_json::from_str(&body_text)
            .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;

        self.parse_response(response_json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_body() {
        let provider = DashScopeEmbeddingProvider::new("test-key");

        // Basic request
        let texts = vec!["hello world".to_string()];
        let options = EmbeddingOptions::new();
        let body = provider.build_request_body(&texts, &options);

        assert_eq!(body["model"], "text-embedding-v4");
        assert_eq!(body["input"][0], "hello world");
        assert_eq!(body["encoding_format"], "float");
        assert!(body.get("dimensions").is_none());

        // With dimension
        let options = EmbeddingOptions::new().with_dimension(512);
        let body = provider.build_request_body(&texts, &options);
        assert_eq!(body["dimensions"], 512);

        // With custom model
        let options = EmbeddingOptions::new().with_model("text-embedding-v3");
        let body = provider.build_request_body(&texts, &options);
        assert_eq!(body["model"], "text-embedding-v3");
    }

    #[test]
    fn test_parse_response() {
        let provider = DashScopeEmbeddingProvider::new("test-key");

        let response = json!({
            "object": "list",
            "data": [
                {
                    "object": "embedding",
                    "index": 0,
                    "embedding": [0.1, 0.2, 0.3]
                }
            ],
            "model": "text-embedding-v4",
            "usage": {
                "prompt_tokens": 3,
                "total_tokens": 3
            }
        });

        let result = provider.parse_response(response).unwrap();
        assert_eq!(result.embeddings.len(), 1);
        assert_eq!(result.embeddings[0].vector, vec![0.1, 0.2, 0.3]);
        assert_eq!(result.embeddings[0].index, 0);
        assert_eq!(result.usage.prompt_tokens, 3);
        assert_eq!(result.model, "text-embedding-v4");
    }
}
