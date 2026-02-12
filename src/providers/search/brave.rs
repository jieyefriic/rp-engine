//! Brave Search API provider
//!
//! Endpoints:
//! - Web Search:  GET https://api.search.brave.com/res/v1/web/search
//! - News Search: GET https://api.search.brave.com/res/v1/news/search
//!
//! Docs: https://api-dashboard.search.brave.com/app/documentation/web-search/query

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, error};

use crate::error::ProviderError;
use super::{SearchProvider, SearchOptions, SearchResponse, SearchResult};

const BRAVE_WEB_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const BRAVE_NEWS_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/news/search";

/// Brave Search provider
pub struct BraveProvider {
    api_key: String,
    client: Client,
}

impl BraveProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    async fn parse_web_response(
        &self,
        query: &str,
        response: reqwest::Response,
    ) -> Result<SearchResponse, ProviderError> {
        let brave_response: BraveResponse = response.json().await
            .map_err(|e| ProviderError::InvalidResponse(format!("Failed to parse Brave response: {}", e)))?;

        let results = brave_response.web
            .map(|web| {
                web.results.into_iter().map(|r| {
                    let site_name = r.meta_url.as_ref()
                        .and_then(|m| m.hostname.clone());
                    let favicon = r.meta_url.as_ref()
                        .and_then(|m| m.favicon.clone());

                    SearchResult {
                        title: r.title,
                        url: r.url,
                        content: r.description.unwrap_or_default(),
                        score: None,
                        raw_content: None,
                        published_date: r.page_age,
                        site_name,
                        favicon,
                    }
                }).collect()
            })
            .unwrap_or_default();

        Ok(SearchResponse {
            query: query.to_string(),
            results,
            answer: None,
        })
    }

    async fn parse_news_response(
        &self,
        query: &str,
        response: reqwest::Response,
    ) -> Result<SearchResponse, ProviderError> {
        let news_response: BraveNewsResponse = response.json().await
            .map_err(|e| ProviderError::InvalidResponse(format!("Failed to parse Brave news response: {}", e)))?;

        let results = news_response.results
            .into_iter()
            .map(|r| {
                SearchResult {
                    title: r.title,
                    url: r.url,
                    content: r.description.unwrap_or_default(),
                    score: None,
                    raw_content: None,
                    published_date: r.age,
                    site_name: r.meta_url.and_then(|m| m.hostname),
                    favicon: None,
                }
            })
            .collect();

        Ok(SearchResponse {
            query: query.to_string(),
            results,
            answer: None,
        })
    }

    fn build_query_params(&self, query: &str, options: &SearchOptions) -> Vec<(String, String)> {
        let mut params = vec![
            ("q".to_string(), query.to_string()),
        ];

        if let Some(max) = options.max_results {
            params.push(("count".to_string(), max.min(20).to_string()));
        }

        // Map unified freshness to Brave format
        if let Some(freshness) = &options.freshness {
            let brave_freshness = match freshness.as_str() {
                "day" => "pd",
                "week" => "pw",
                "month" => "pm",
                "year" => "py",
                other => other,
            };
            params.push(("freshness".to_string(), brave_freshness.to_string()));
        }

        if let Some(country) = &options.country {
            params.push(("country".to_string(), country.clone()));
        }

        if let Some(lang) = &options.language {
            params.push(("search_lang".to_string(), lang.clone()));
        }

        params
    }
}

#[async_trait]
impl SearchProvider for BraveProvider {
    fn name(&self) -> &str { "brave" }

    async fn search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResponse, ProviderError> {
        let params = self.build_query_params(query, &options);
        let is_news = options.search_type.as_deref() == Some("news");
        let url = if is_news { BRAVE_NEWS_SEARCH_URL } else { BRAVE_WEB_SEARCH_URL };

        debug!(provider = "brave", query = %query, search_type = if is_news { "news" } else { "web" }, "Executing search");

        let response = self.client
            .get(url)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .query(&params)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(format!("Brave request failed: {}", e)))?;

        let status = response.status().as_u16();
        if status != 200 {
            let body = response.text().await.unwrap_or_default();
            error!(provider = "brave", status = %status, body = %body, "Brave API error");
            return match status {
                401 => Err(ProviderError::AuthenticationFailed),
                429 => Err(ProviderError::RateLimited),
                _ => Err(ProviderError::ApiError { status, message: body }),
            };
        }

        if is_news {
            self.parse_news_response(query, response).await
        } else {
            self.parse_web_response(query, response).await
        }
    }
}

// ============================================================================
// Brave API Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    #[serde(default)]
    results: Vec<BraveWebResult>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResult {
    title: String,
    url: String,
    description: Option<String>,
    page_age: Option<String>,
    meta_url: Option<BraveMetaUrl>,
}

#[derive(Debug, Deserialize)]
struct BraveMetaUrl {
    hostname: Option<String>,
    favicon: Option<String>,
}

// News search types

#[derive(Debug, Deserialize)]
struct BraveNewsResponse {
    #[serde(default)]
    results: Vec<BraveNewsResult>,
}

#[derive(Debug, Deserialize)]
struct BraveNewsResult {
    title: String,
    url: String,
    description: Option<String>,
    /// Relative time, e.g. "5 hours ago"
    age: Option<String>,
    meta_url: Option<BraveMetaUrl>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_query_params_defaults() {
        let provider = BraveProvider::new("test-key");
        let params = provider.build_query_params("rust programming", &SearchOptions::default());
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], ("q".to_string(), "rust programming".to_string()));
    }

    #[test]
    fn test_build_query_params_with_options() {
        let provider = BraveProvider::new("test-key");
        let options = SearchOptions {
            max_results: Some(10),
            freshness: Some("week".to_string()),
            country: Some("US".to_string()),
            ..Default::default()
        };
        let params = provider.build_query_params("test", &options);
        assert!(params.iter().any(|(k, v)| k == "count" && v == "10"));
        assert!(params.iter().any(|(k, v)| k == "freshness" && v == "pw"));
        assert!(params.iter().any(|(k, v)| k == "country" && v == "US"));
    }

    #[test]
    fn test_brave_max_results_cap() {
        let provider = BraveProvider::new("test-key");
        let options = SearchOptions {
            max_results: Some(50), // exceeds Brave max of 20
            ..Default::default()
        };
        let params = provider.build_query_params("test", &options);
        assert!(params.iter().any(|(k, v)| k == "count" && v == "20"));
    }

    #[test]
    fn test_parse_brave_response() {
        let json = r#"{
            "web": {
                "results": [
                    {
                        "title": "Brave Page",
                        "url": "https://example.com",
                        "description": "A test description",
                        "page_age": "2 days ago",
                        "meta_url": {
                            "hostname": "example.com",
                            "favicon": "https://example.com/favicon.ico"
                        }
                    }
                ]
            }
        }"#;

        let response: BraveResponse = serde_json::from_str(json).unwrap();
        let web = response.web.unwrap();
        assert_eq!(web.results.len(), 1);
        assert_eq!(web.results[0].title, "Brave Page");
        assert_eq!(web.results[0].meta_url.as_ref().unwrap().hostname.as_deref(), Some("example.com"));
    }

    #[test]
    fn test_parse_brave_news_response() {
        let json = r#"{
            "results": [
                {
                    "title": "Breaking News: Rust 2.0",
                    "url": "https://news.example.com/rust",
                    "description": "Major release announcement",
                    "age": "2 hours ago",
                    "meta_url": {
                        "hostname": "news.example.com"
                    }
                },
                {
                    "title": "Rust Adoption Grows",
                    "url": "https://tech.example.com/rust-growth",
                    "description": "Enterprise adoption increases",
                    "age": "5 hours ago"
                }
            ]
        }"#;

        let response: BraveNewsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.results.len(), 2);
        assert_eq!(response.results[0].title, "Breaking News: Rust 2.0");
        assert_eq!(response.results[0].age.as_deref(), Some("2 hours ago"));
        assert_eq!(
            response.results[0].meta_url.as_ref().unwrap().hostname.as_deref(),
            Some("news.example.com")
        );
        assert!(response.results[1].meta_url.is_none());
    }

    #[test]
    fn test_parse_brave_empty_response() {
        let json = r#"{}"#;
        let response: BraveResponse = serde_json::from_str(json).unwrap();
        assert!(response.web.is_none());
    }
}
