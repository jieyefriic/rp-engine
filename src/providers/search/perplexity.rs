//! Perplexity Sonar Search API provider
//!
//! API: POST https://api.perplexity.ai/chat/completions (OpenAI-compatible)
//! Docs: https://docs.perplexity.ai/
//!
//! Unlike other search providers, Perplexity returns a synthesized answer
//! with citations rather than raw search results. The search_results field
//! provides metadata about sources used.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, error};

use crate::error::ProviderError;
use super::{SearchProvider, SearchOptions, SearchResponse, SearchResult};

const PERPLEXITY_API_URL: &str = "https://api.perplexity.ai/chat/completions";
const DEFAULT_MODEL: &str = "sonar";

/// Perplexity Sonar search provider
pub struct PerplexitySearchProvider {
    api_key: String,
    client: Client,
}

impl PerplexitySearchProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    fn build_request(&self, query: &str, options: &SearchOptions) -> serde_json::Value {
        let model = options.model.as_deref().unwrap_or(DEFAULT_MODEL);

        let mut body = serde_json::json!({
            "model": model,
            "messages": [
                {
                    "role": "user",
                    "content": query
                }
            ]
        });

        // Map unified freshness to Perplexity search_recency_filter
        if let Some(freshness) = &options.freshness {
            let filter = match freshness.as_str() {
                "day" => "day",
                "week" => "week",
                "month" => "month",
                "year" => "year",
                other => other,
            };
            body["search_recency_filter"] = serde_json::json!(filter);
        }

        // Domain filters
        let mut domain_filter: Vec<String> = Vec::new();
        for d in &options.include_domains {
            domain_filter.push(d.clone());
        }
        for d in &options.exclude_domains {
            domain_filter.push(format!("-{}", d));
        }
        if !domain_filter.is_empty() {
            body["search_domain_filter"] = serde_json::json!(domain_filter);
        }

        body
    }
}

#[async_trait]
impl SearchProvider for PerplexitySearchProvider {
    fn name(&self) -> &str { "perplexity" }

    async fn search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResponse, ProviderError> {
        let request_body = self.build_request(query, &options);

        debug!(provider = "perplexity", query = %query, "Executing search");

        let response = self.client
            .post(PERPLEXITY_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(format!("Perplexity request failed: {}", e)))?;

        let status = response.status().as_u16();
        if status != 200 {
            let body = response.text().await.unwrap_or_default();
            error!(provider = "perplexity", status = %status, body = %body, "Perplexity API error");
            return match status {
                401 => Err(ProviderError::AuthenticationFailed),
                429 => Err(ProviderError::RateLimited),
                _ => Err(ProviderError::ApiError { status, message: body }),
            };
        }

        let pplx_response: PerplexityResponse = response.json().await
            .map_err(|e| ProviderError::InvalidResponse(format!("Failed to parse Perplexity response: {}", e)))?;

        // Extract answer from choices
        let answer = pplx_response.choices.first()
            .map(|c| c.message.content.clone());

        // Build results from search_results + citations
        let mut results: Vec<SearchResult> = Vec::new();

        if let Some(search_results) = pplx_response.search_results {
            for sr in search_results {
                results.push(SearchResult {
                    title: sr.title,
                    url: sr.url,
                    content: sr.snippet.unwrap_or_default(),
                    score: None,
                    raw_content: None,
                    published_date: sr.date,
                    site_name: None,
                    favicon: None,
                });
            }
        } else if let Some(citations) = &pplx_response.citations {
            // Fallback: if no search_results but citations exist, create minimal results
            for url in citations {
                results.push(SearchResult {
                    title: String::new(),
                    url: url.clone(),
                    content: String::new(),
                    score: None,
                    raw_content: None,
                    published_date: None,
                    site_name: None,
                    favicon: None,
                });
            }
        }

        Ok(SearchResponse {
            query: query.to_string(),
            results,
            answer,
        })
    }
}

// ============================================================================
// Perplexity API Types (OpenAI-compatible)
// ============================================================================

#[derive(Debug, Deserialize)]
struct PerplexityResponse {
    #[serde(default)]
    choices: Vec<PerplexityChoice>,
    #[serde(default)]
    citations: Option<Vec<String>>,
    #[serde(default)]
    search_results: Option<Vec<PerplexitySearchResult>>,
}

#[derive(Debug, Deserialize)]
struct PerplexityChoice {
    message: PerplexityMessage,
}

#[derive(Debug, Deserialize)]
struct PerplexityMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct PerplexitySearchResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_defaults() {
        let provider = PerplexitySearchProvider::new("test-key");
        let req = provider.build_request("test query", &SearchOptions::default());
        assert_eq!(req["model"], "sonar");
        assert_eq!(req["messages"][0]["content"], "test query");
        assert!(req.get("search_recency_filter").is_none());
    }

    #[test]
    fn test_build_request_with_options() {
        let provider = PerplexitySearchProvider::new("test-key");
        let options = SearchOptions {
            freshness: Some("week".to_string()),
            model: Some("sonar-pro".to_string()),
            include_domains: vec!["example.com".to_string()],
            exclude_domains: vec!["spam.com".to_string()],
            ..Default::default()
        };
        let req = provider.build_request("test", &options);
        assert_eq!(req["model"], "sonar-pro");
        assert_eq!(req["search_recency_filter"], "week");
        let filters = req["search_domain_filter"].as_array().unwrap();
        assert_eq!(filters[0], "example.com");
        assert_eq!(filters[1], "-spam.com");
    }

    #[test]
    fn test_parse_perplexity_response() {
        let json = r#"{
            "id": "test-id",
            "model": "sonar",
            "choices": [
                {
                    "index": 0,
                    "finish_reason": "stop",
                    "message": {
                        "role": "assistant",
                        "content": "Rust is a systems programming language [1][2]."
                    }
                }
            ],
            "citations": [
                "https://www.rust-lang.org/",
                "https://doc.rust-lang.org/"
            ],
            "search_results": [
                {
                    "title": "Rust Programming Language",
                    "url": "https://www.rust-lang.org/",
                    "snippet": "A language empowering everyone to build reliable software.",
                    "date": "2024-01-15"
                },
                {
                    "title": "Rust Documentation",
                    "url": "https://doc.rust-lang.org/",
                    "snippet": "Official Rust documentation."
                }
            ]
        }"#;

        let response: PerplexityResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.choices.len(), 1);
        assert!(response.choices[0].message.content.contains("Rust"));
        assert_eq!(response.citations.as_ref().unwrap().len(), 2);
        assert_eq!(response.search_results.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_parse_perplexity_response_no_search_results() {
        let json = r#"{
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "An answer"
                    }
                }
            ],
            "citations": ["https://example.com"]
        }"#;

        let response: PerplexityResponse = serde_json::from_str(json).unwrap();
        assert!(response.search_results.is_none());
        assert_eq!(response.citations.as_ref().unwrap().len(), 1);
    }
}
