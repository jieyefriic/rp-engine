//! Bocha/博查 Search API provider
//!
//! Endpoints:
//! - Web Search: POST https://api.bochaai.com/v1/web-search
//! - AI Search:  POST https://api.bochaai.com/v1/ai-search (enhanced with structured cards)
//!
//! Docs: https://open.bochaai.com/

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::error::ProviderError;
use super::{SearchProvider, SearchOptions, SearchResponse, SearchResult};

const BOCHA_WEB_SEARCH_URL: &str = "https://api.bochaai.com/v1/web-search";
const BOCHA_AI_SEARCH_URL: &str = "https://api.bochaai.com/v1/ai-search";

/// Bocha/博查 search provider
pub struct BochaProvider {
    api_key: String,
    client: Client,
}

impl BochaProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    fn build_request(&self, query: &str, options: &SearchOptions) -> BochaRequest {
        let freshness = options.freshness.as_ref().map(|f| {
            match f.as_str() {
                "day" => "oneDay".to_string(),
                "week" => "oneWeek".to_string(),
                "month" => "oneMonth".to_string(),
                "year" => "oneYear".to_string(),
                other => other.to_string(),
            }
        }).unwrap_or_else(|| "noLimit".to_string());

        BochaRequest {
            query: query.to_string(),
            freshness,
            summary: options.include_answer,
            count: options.max_results.unwrap_or(10),
        }
    }
}

#[async_trait]
impl SearchProvider for BochaProvider {
    fn name(&self) -> &str { "bocha" }

    async fn search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResponse, ProviderError> {
        let request_body = self.build_request(query, &options);

        // Select endpoint based on search_type
        let is_ai_search = options.search_type.as_deref() == Some("ai");
        let url = if is_ai_search { BOCHA_AI_SEARCH_URL } else { BOCHA_WEB_SEARCH_URL };

        debug!(provider = "bocha", query = %query, search_type = if is_ai_search { "ai" } else { "web" }, "Executing search");

        let response = self.client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(format!("Bocha request failed: {}", e)))?;

        let status = response.status().as_u16();
        if status != 200 {
            let body = response.text().await.unwrap_or_default();
            error!(provider = "bocha", status = %status, body = %body, "Bocha API error");
            return match status {
                401 => Err(ProviderError::AuthenticationFailed),
                429 => Err(ProviderError::RateLimited),
                _ => Err(ProviderError::ApiError { status, message: body }),
            };
        }

        let raw: serde_json::Value = response.json().await
            .map_err(|e| ProviderError::InvalidResponse(format!("Failed to parse Bocha response: {}", e)))?;

        // Bocha may wrap response in a "data" field or return directly
        let bocha_response: BochaResponse = if raw.get("data").is_some() {
            serde_json::from_value(raw["data"].clone())
        } else {
            serde_json::from_value(raw.clone())
        }.map_err(|e| ProviderError::InvalidResponse(format!("Failed to parse Bocha response body: {}", e)))?;

        // Convert to unified format
        let results = bocha_response.web_pages
            .map(|pages| {
                pages.value.into_iter().map(|r| SearchResult {
                    title: r.name,
                    url: r.url,
                    content: r.summary.unwrap_or(r.snippet),
                    score: None,
                    raw_content: None,
                    published_date: r.date_published,
                    site_name: r.site_name,
                    favicon: r.site_icon,
                }).collect()
            })
            .unwrap_or_default();

        // AI search may return an answer field
        let answer = if is_ai_search {
            raw.get("data")
                .and_then(|d| d.get("answer"))
                .or_else(|| raw.get("answer"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        } else {
            None
        };

        Ok(SearchResponse {
            query: query.to_string(),
            results,
            answer,
        })
    }
}

// ============================================================================
// Bocha API Types
// ============================================================================

#[derive(Debug, Serialize)]
struct BochaRequest {
    query: String,
    freshness: String,
    summary: bool,
    count: u32,
}

#[derive(Debug, Deserialize)]
struct BochaResponse {
    #[serde(rename = "webPages")]
    web_pages: Option<BochaWebPages>,
}

#[derive(Debug, Deserialize)]
struct BochaWebPages {
    #[serde(default)]
    value: Vec<BochaWebResult>,
}

#[derive(Debug, Deserialize)]
struct BochaWebResult {
    name: String,
    url: String,
    snippet: String,
    summary: Option<String>,
    #[serde(rename = "siteName")]
    site_name: Option<String>,
    #[serde(rename = "siteIcon")]
    site_icon: Option<String>,
    #[serde(rename = "datePublished")]
    date_published: Option<String>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_defaults() {
        let provider = BochaProvider::new("test-key");
        let req = provider.build_request("test query", &SearchOptions::default());
        assert_eq!(req.query, "test query");
        assert_eq!(req.freshness, "noLimit");
        assert!(!req.summary);
        assert_eq!(req.count, 10);
    }

    #[test]
    fn test_build_request_with_options() {
        let provider = BochaProvider::new("test-key");
        let options = SearchOptions {
            max_results: Some(5),
            freshness: Some("week".to_string()),
            include_answer: true,
            ..Default::default()
        };
        let req = provider.build_request("test", &options);
        assert_eq!(req.freshness, "oneWeek");
        assert!(req.summary);
        assert_eq!(req.count, 5);
    }

    #[test]
    fn test_parse_bocha_response() {
        let json = r#"{
            "webPages": {
                "value": [
                    {
                        "name": "Test Page",
                        "url": "https://example.com",
                        "snippet": "Short snippet",
                        "summary": "Longer summary text",
                        "siteName": "Example",
                        "siteIcon": "https://example.com/icon.png",
                        "datePublished": "2024-07-22T00:00:00+08:00"
                    }
                ]
            }
        }"#;

        let response: BochaResponse = serde_json::from_str(json).unwrap();
        let pages = response.web_pages.unwrap();
        assert_eq!(pages.value.len(), 1);
        assert_eq!(pages.value[0].name, "Test Page");
        assert_eq!(pages.value[0].summary.as_deref(), Some("Longer summary text"));
    }

    #[test]
    fn test_parse_bocha_ai_search_response() {
        // AI search returns same webPages structure but may include an answer
        let json = r#"{
            "data": {
                "webPages": {
                    "value": [
                        {
                            "name": "AI Search Result",
                            "url": "https://example.com",
                            "snippet": "AI snippet"
                        }
                    ]
                },
                "answer": "This is an AI-generated summary of the search results."
            }
        }"#;

        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        let bocha: BochaResponse = serde_json::from_value(raw["data"].clone()).unwrap();
        assert_eq!(bocha.web_pages.unwrap().value[0].name, "AI Search Result");

        // Verify answer extraction
        let answer = raw.get("data")
            .and_then(|d| d.get("answer"))
            .and_then(|v| v.as_str());
        assert_eq!(answer, Some("This is an AI-generated summary of the search results."));
    }

    #[test]
    fn test_parse_bocha_wrapped_response() {
        let json = r#"{
            "data": {
                "webPages": {
                    "value": [
                        {
                            "name": "Wrapped Page",
                            "url": "https://example.com",
                            "snippet": "A snippet"
                        }
                    ]
                }
            }
        }"#;

        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        let bocha: BochaResponse = serde_json::from_value(raw["data"].clone()).unwrap();
        assert_eq!(bocha.web_pages.unwrap().value[0].name, "Wrapped Page");
    }
}
