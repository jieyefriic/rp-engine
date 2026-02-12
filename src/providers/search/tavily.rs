//! Tavily Search API provider
//!
//! API: POST https://api.tavily.com/search
//! Docs: https://docs.tavily.com/

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::error::ProviderError;
use super::{SearchProvider, SearchOptions, SearchResponse, SearchResult};

const TAVILY_API_URL: &str = "https://api.tavily.com/search";

/// Tavily search provider
pub struct TavilyProvider {
    api_key: String,
    client: Client,
}

impl TavilyProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    fn build_request(&self, query: &str, options: &SearchOptions) -> TavilyRequest {
        let mut req = TavilyRequest {
            query: query.to_string(),
            search_depth: options.search_depth.clone().unwrap_or_else(|| "basic".to_string()),
            max_results: options.max_results.unwrap_or(5),
            include_answer: options.include_answer,
            include_raw_content: options.include_raw_content,
            include_domains: if options.include_domains.is_empty() { None } else { Some(options.include_domains.clone()) },
            exclude_domains: if options.exclude_domains.is_empty() { None } else { Some(options.exclude_domains.clone()) },
            time_range: None,
            topic: None,
            country: options.country.clone(),
        };

        // Map unified freshness to Tavily time_range
        if let Some(freshness) = &options.freshness {
            req.time_range = Some(match freshness.as_str() {
                "day" => "day".to_string(),
                "week" => "week".to_string(),
                "month" => "month".to_string(),
                "year" => "year".to_string(),
                other => other.to_string(),
            });
        }

        // Map search_type to Tavily topic
        if let Some(search_type) = &options.search_type {
            match search_type.as_str() {
                "news" => req.topic = Some("news".to_string()),
                _ => {} // "web" is default, no topic needed
            }
        }

        req
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &str { "tavily" }

    async fn search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResponse, ProviderError> {
        let request_body = self.build_request(query, &options);

        debug!(provider = "tavily", query = %query, "Executing search");

        let response = self.client
            .post(TAVILY_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(format!("Tavily request failed: {}", e)))?;

        let status = response.status().as_u16();
        if status != 200 {
            let body = response.text().await.unwrap_or_default();
            error!(provider = "tavily", status = %status, body = %body, "Tavily API error");
            return match status {
                401 => Err(ProviderError::AuthenticationFailed),
                429 => Err(ProviderError::RateLimited),
                _ => Err(ProviderError::ApiError { status, message: body }),
            };
        }

        let tavily_response: TavilyResponse = response.json().await
            .map_err(|e| ProviderError::InvalidResponse(format!("Failed to parse Tavily response: {}", e)))?;

        // Convert to unified format
        let results = tavily_response.results.into_iter().map(|r| SearchResult {
            title: r.title,
            url: r.url,
            content: r.content,
            score: Some(r.score),
            raw_content: r.raw_content,
            published_date: None,
            site_name: None,
            favicon: r.favicon,
        }).collect();

        Ok(SearchResponse {
            query: tavily_response.query,
            results,
            answer: tavily_response.answer,
        })
    }
}

// ============================================================================
// Tavily API Types
// ============================================================================

#[derive(Debug, Serialize)]
struct TavilyRequest {
    query: String,
    search_depth: String,
    max_results: u32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    include_answer: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    include_raw_content: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exclude_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    query: String,
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
    #[serde(default)]
    score: f64,
    raw_content: Option<String>,
    favicon: Option<String>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_defaults() {
        let provider = TavilyProvider::new("test-key");
        let req = provider.build_request("rust programming", &SearchOptions::default());
        assert_eq!(req.query, "rust programming");
        assert_eq!(req.search_depth, "basic");
        assert_eq!(req.max_results, 5);
        assert!(!req.include_answer);
        assert!(req.time_range.is_none());
    }

    #[test]
    fn test_build_request_with_options() {
        let provider = TavilyProvider::new("test-key");
        let options = SearchOptions {
            max_results: Some(10),
            freshness: Some("week".to_string()),
            include_answer: true,
            search_depth: Some("advanced".to_string()),
            include_domains: vec!["example.com".to_string()],
            ..Default::default()
        };
        let req = provider.build_request("test", &options);
        assert_eq!(req.max_results, 10);
        assert_eq!(req.search_depth, "advanced");
        assert_eq!(req.time_range.as_deref(), Some("week"));
        assert!(req.include_answer);
        assert_eq!(req.include_domains.unwrap(), vec!["example.com"]);
    }

    #[test]
    fn test_build_request_news_topic() {
        let provider = TavilyProvider::new("test-key");
        let options = SearchOptions {
            search_type: Some("news".to_string()),
            ..Default::default()
        };
        let req = provider.build_request("breaking news", &options);
        assert_eq!(req.topic.as_deref(), Some("news"));
    }

    #[test]
    fn test_build_request_web_topic_default() {
        let provider = TavilyProvider::new("test-key");
        let options = SearchOptions {
            search_type: Some("web".to_string()),
            ..Default::default()
        };
        let req = provider.build_request("test", &options);
        assert!(req.topic.is_none()); // "web" is default, no topic needed
    }

    #[test]
    fn test_parse_tavily_response() {
        let json = r#"{
            "query": "test query",
            "answer": "This is an answer",
            "results": [
                {
                    "title": "Test Page",
                    "url": "https://example.com",
                    "content": "Test content snippet",
                    "score": 0.95,
                    "raw_content": null,
                    "favicon": "https://example.com/favicon.ico"
                }
            ],
            "response_time": 1.5
        }"#;

        let response: TavilyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.query, "test query");
        assert_eq!(response.answer.as_deref(), Some("This is an answer"));
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].title, "Test Page");
        assert_eq!(response.results[0].score, 0.95);
    }
}
