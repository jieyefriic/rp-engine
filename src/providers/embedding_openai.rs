//! OpenAI Embedding Provider
//!
//! API: https://api.openai.com/v1/embeddings
//! Default model: text-embedding-3-large
//! Dimensions: 256-3072 (model dependent)

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::error::ProviderError;
use super::embedding::{EmbeddingProvider, EmbeddingOptions, EmbeddingResponse, Embedding, EmbeddingUsage};

const API_URL: &str = "https://api.openai.com/v1/embeddings";
const DEFAULT_MODEL: &str = "text-embedding-3-large";

/// OpenAI embedding provider
pub struct OpenAIEmbeddingProvider {
    api_key: String,
    client: Client,
}

impl OpenAIEmbeddingProvider {
    /// Create a new OpenAI embedding provider
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

        // Add dimension if specified
        // text-embedding-3-small: 512-1536 (default 1536)
        // text-embedding-3-large: 256-3072 (default 3072)
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
impl EmbeddingProvider for OpenAIEmbeddingProvider {
    fn name(&self) -> &str {
        "openai"
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
        let provider = OpenAIEmbeddingProvider::new("test-key");

        // Basic request
        let texts = vec!["hello world".to_string()];
        let options = EmbeddingOptions::new();
        let body = provider.build_request_body(&texts, &options);

        assert_eq!(body["model"], "text-embedding-3-large");
        assert_eq!(body["input"][0], "hello world");
        assert_eq!(body["encoding_format"], "float");
        assert!(body.get("dimensions").is_none());

        // With dimension
        let options = EmbeddingOptions::new().with_dimension(1024);
        let body = provider.build_request_body(&texts, &options);
        assert_eq!(body["dimensions"], 1024);

        // With custom model
        let options = EmbeddingOptions::new().with_model("text-embedding-3-small");
        let body = provider.build_request_body(&texts, &options);
        assert_eq!(body["model"], "text-embedding-3-small");
    }

    #[test]
    fn test_parse_response() {
        let provider = OpenAIEmbeddingProvider::new("test-key");

        let response = json!({
            "object": "list",
            "data": [
                {
                    "object": "embedding",
                    "index": 0,
                    "embedding": [0.1, 0.2, 0.3, 0.4]
                },
                {
                    "object": "embedding",
                    "index": 1,
                    "embedding": [0.5, 0.6, 0.7, 0.8]
                }
            ],
            "model": "text-embedding-3-large",
            "usage": {
                "prompt_tokens": 6,
                "total_tokens": 6
            }
        });

        let result = provider.parse_response(response).unwrap();
        assert_eq!(result.embeddings.len(), 2);
        assert_eq!(result.embeddings[0].vector, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(result.embeddings[0].index, 0);
        assert_eq!(result.embeddings[1].vector, vec![0.5, 0.6, 0.7, 0.8]);
        assert_eq!(result.embeddings[1].index, 1);
        assert_eq!(result.usage.prompt_tokens, 6);
        assert_eq!(result.model, "text-embedding-3-large");
    }
}
