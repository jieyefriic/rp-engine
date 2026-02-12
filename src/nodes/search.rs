//! Search node executor
//!
//! Executes web searches using configured search providers (Tavily, Brave, Bocha, Perplexity).
//!
//! # YAML Usage
//!
//! ```yaml
//! - id: web_search
//!   type: search
//!   config:
//!     provider: tavily
//!     variables:
//!       query: "start.user_query"
//!     operation:
//!       max_results: 5
//!       freshness: week
//!       include_answer: true
//! ```
//!
//! # Output
//!
//! - `results`: Array of search result objects `[{title, url, content, score, ...}]`
//! - `answer`: AI-generated summary (if available and requested)
//! - `query`: The actual search query used

use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info};

use crate::schema::YamlNode;
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use crate::providers::search::SearchOptions;
use super::NodeExecutor;

/// Search node executor
pub struct SearchExecutor;

#[async_trait]
impl NodeExecutor for SearchExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_config = &node.config;
        let node_id = &node.id;

        // 1. Get search provider
        let provider_id = node_config.provider.as_ref()
            .ok_or_else(|| EngineError::SearchError(
                format!("Search node '{}' missing 'provider' in config", node_id)
            ))?;

        let provider = config.search_providers.get(provider_id)
            .ok_or_else(|| EngineError::SearchError(
                format!(
                    "Search provider '{}' not found for node '{}'. Available: {:?}",
                    provider_id, node_id, config.search_providers.names()
                )
            ))?;

        // 2. Resolve variables
        let resolved = super::resolve_variables(&node_config.variables, state);

        // 3. Get query from resolved variables
        let query = resolved.get("query")
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                other => Some(other.to_string()),
            })
            .ok_or_else(|| EngineError::SearchError(
                format!("Search node '{}' missing 'query' variable", node_id)
            ))?;

        if query.is_empty() || query == "null" {
            return Err(EngineError::SearchError(
                format!("Search node '{}' has empty query", node_id)
            ));
        }

        // 4. Build SearchOptions from operation config
        let operation = &node_config.operation;
        let options = build_search_options(operation);

        debug!(
            node = %node_id,
            provider = %provider_id,
            query = %query,
            "Executing search node"
        );

        // 5. Execute search
        let response = provider.search(&query, options).await
            .map_err(|e| EngineError::SearchError(
                format!("Search failed for node '{}': {}", node_id, e)
            ))?;

        info!(
            node = %node_id,
            provider = %provider_id,
            results_count = response.results.len(),
            has_answer = response.answer.is_some(),
            "Search completed"
        );

        // 6. Build outputs
        let mut outputs = HashMap::new();

        // results as JSON array
        let results_value = serde_json::to_value(&response.results)
            .map_err(|e| EngineError::SearchError(format!("Failed to serialize results: {}", e)))?;
        outputs.insert("results".to_string(), results_value);

        // answer (if available)
        if let Some(answer) = &response.answer {
            outputs.insert("answer".to_string(), Value::String(answer.clone()));
        } else {
            outputs.insert("answer".to_string(), Value::Null);
        }

        // query echo
        outputs.insert("query".to_string(), Value::String(response.query));

        Ok(outputs)
    }
}

/// Build SearchOptions from the node's operation config
fn build_search_options(operation: &HashMap<String, Value>) -> SearchOptions {
    let mut options = SearchOptions::default();

    if let Some(v) = operation.get("max_results").and_then(|v| v.as_u64()) {
        options.max_results = Some(v as u32);
    }

    if let Some(v) = operation.get("freshness").and_then(|v| v.as_str()) {
        options.freshness = Some(v.to_string());
    }

    if let Some(v) = operation.get("search_depth").and_then(|v| v.as_str()) {
        options.search_depth = Some(v.to_string());
    }

    if let Some(v) = operation.get("include_answer").and_then(|v| v.as_bool()) {
        options.include_answer = v;
    }

    if let Some(v) = operation.get("include_raw_content").and_then(|v| v.as_bool()) {
        options.include_raw_content = v;
    }

    if let Some(v) = operation.get("country").and_then(|v| v.as_str()) {
        options.country = Some(v.to_string());
    }

    if let Some(v) = operation.get("language").and_then(|v| v.as_str()) {
        options.language = Some(v.to_string());
    }

    if let Some(v) = operation.get("model").and_then(|v| v.as_str()) {
        options.model = Some(v.to_string());
    }

    if let Some(arr) = operation.get("include_domains").and_then(|v| v.as_array()) {
        options.include_domains = arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }

    if let Some(arr) = operation.get("exclude_domains").and_then(|v| v.as_array()) {
        options.exclude_domains = arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }

    if let Some(v) = operation.get("search_type").and_then(|v| v.as_str()) {
        options.search_type = Some(v.to_string());
    }

    options
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_search_options_empty() {
        let operation = HashMap::new();
        let options = build_search_options(&operation);
        assert!(options.max_results.is_none());
        assert!(options.freshness.is_none());
        assert!(!options.include_answer);
    }

    #[test]
    fn test_build_search_options_with_search_type() {
        let mut operation = HashMap::new();
        operation.insert("search_type".to_string(), Value::String("news".to_string()));
        operation.insert("max_results".to_string(), Value::Number(5.into()));

        let options = build_search_options(&operation);
        assert_eq!(options.search_type.as_deref(), Some("news"));
        assert_eq!(options.max_results, Some(5));
    }

    #[test]
    fn test_build_search_options_full() {
        let mut operation = HashMap::new();
        operation.insert("max_results".to_string(), Value::Number(10.into()));
        operation.insert("freshness".to_string(), Value::String("week".to_string()));
        operation.insert("search_depth".to_string(), Value::String("advanced".to_string()));
        operation.insert("include_answer".to_string(), Value::Bool(true));
        operation.insert("country".to_string(), Value::String("US".to_string()));
        operation.insert("include_domains".to_string(), serde_json::json!(["example.com"]));
        operation.insert("exclude_domains".to_string(), serde_json::json!(["spam.com"]));

        let options = build_search_options(&operation);
        assert_eq!(options.max_results, Some(10));
        assert_eq!(options.freshness.as_deref(), Some("week"));
        assert_eq!(options.search_depth.as_deref(), Some("advanced"));
        assert!(options.include_answer);
        assert_eq!(options.country.as_deref(), Some("US"));
        assert_eq!(options.include_domains, vec!["example.com"]);
        assert_eq!(options.exclude_domains, vec!["spam.com"]);
    }
}
