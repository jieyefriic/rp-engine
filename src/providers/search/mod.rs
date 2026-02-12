//! Search provider implementations for web search APIs
//!
//! Supports 4 search providers: Tavily, Brave, Bocha, Perplexity Sonar
//! All search providers implement the SearchProvider trait with a search() method.

pub mod tavily;
pub mod brave;
pub mod bocha;
pub mod perplexity;

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

// Re-export providers
pub use tavily::TavilyProvider;
pub use brave::BraveProvider;
pub use bocha::BochaProvider;
pub use perplexity::PerplexitySearchProvider;

// ============================================================================
// Search Types
// ============================================================================

/// Options for search queries (unified across all providers)
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Maximum number of results to return
    pub max_results: Option<u32>,

    /// Time freshness filter: "day", "week", "month", "year"
    pub freshness: Option<String>,

    /// Only include results from these domains
    pub include_domains: Vec<String>,

    /// Exclude results from these domains
    pub exclude_domains: Vec<String>,

    /// Search depth (provider-specific, e.g. Tavily "basic"/"advanced")
    pub search_depth: Option<String>,

    /// Whether to include an AI-generated answer/summary
    pub include_answer: bool,

    /// Whether to include raw/full page content
    pub include_raw_content: bool,

    /// Country code for geo-biased results (e.g. "US", "CN")
    pub country: Option<String>,

    /// Language code for results (e.g. "en", "zh")
    pub language: Option<String>,

    /// Provider-specific model override (used by Perplexity: "sonar", "sonar-pro")
    pub model: Option<String>,

    /// Search type: "web" (default), "news" (Tavily/Brave), "ai" (Bocha)
    /// - Tavily: "news" sets topic=news (news-optimized search)
    /// - Brave: "news" switches to /news/search endpoint
    /// - Bocha: "ai" switches to /v1/ai-search endpoint (AI-enhanced results)
    pub search_type: Option<String>,
}

/// A single search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Page title
    pub title: String,

    /// Page URL
    pub url: String,

    /// Snippet / content summary
    pub content: String,

    /// Relevance score (0.0 - 1.0, provider-dependent)
    pub score: Option<f64>,

    /// Full page content (if requested)
    pub raw_content: Option<String>,

    /// Publication date (ISO 8601 or human-readable)
    pub published_date: Option<String>,

    /// Source site name
    pub site_name: Option<String>,

    /// Favicon URL
    pub favicon: Option<String>,
}

/// Unified search response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    /// The query that was executed
    pub query: String,

    /// Search results
    pub results: Vec<SearchResult>,

    /// AI-generated answer/summary (if available)
    pub answer: Option<String>,
}

// ============================================================================
// Search Provider Trait
// ============================================================================

/// Trait for search providers — all providers implement this
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Get the provider name (e.g., "tavily", "brave")
    fn name(&self) -> &str;

    /// Execute a search query
    async fn search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResponse, ProviderError>;
}

// ============================================================================
// Search Provider Registry
// ============================================================================

/// Registry for managing multiple search providers
#[derive(Clone, Default)]
pub struct SearchProviderRegistry {
    providers: HashMap<String, Arc<dyn SearchProvider>>,
}

impl SearchProviderRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self { providers: HashMap::new() }
    }

    /// Register a search provider
    pub fn register(&mut self, name: impl Into<String>, provider: Arc<dyn SearchProvider>) {
        self.providers.insert(name.into(), provider);
    }

    /// Get a search provider by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn SearchProvider>> {
        self.providers.get(name).cloned()
    }

    /// Check if a search provider exists
    pub fn has(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get all registered search provider names
    pub fn names(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}

/// Helper to create a search registry from API keys
pub fn create_search_registry(
    tavily_key: Option<String>,
    brave_key: Option<String>,
    bocha_key: Option<String>,
    perplexity_key: Option<String>,
) -> SearchProviderRegistry {
    let mut registry = SearchProviderRegistry::new();

    if let Some(key) = tavily_key {
        registry.register("tavily", Arc::new(TavilyProvider::new(key)));
    }

    if let Some(key) = brave_key {
        registry.register("brave", Arc::new(BraveProvider::new(key)));
    }

    if let Some(key) = bocha_key {
        registry.register("bocha", Arc::new(BochaProvider::new(key)));
    }

    if let Some(key) = perplexity_key {
        registry.register("perplexity", Arc::new(PerplexitySearchProvider::new(key)));
    }

    registry
}
