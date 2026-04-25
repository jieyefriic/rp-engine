//! EvoMap A2A (Agent-to-Agent) protocol client for the GEP self-evolution engine.
//!
//! Communicates with the EvoMap Hub (`https://evomap.ai`) via the GEP protocol:
//! - `POST /a2a/hello`          — register / heartbeat
//! - `POST /a2a/memory/record`  — record execution outcome for evolution memory
//! - `POST /a2a/memory/recall`  — recall past experience
//! - `POST /a2a/publish`        — publish evolution assets (Gene / Capsule)
//!
//! Reference: <https://evomap.ai/learn/connect-ai-agent>

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::error::EngineError;

// ============================================================================
// Configuration
// ============================================================================

/// EvoMap connection configuration.
///
/// Register via `EngineBuilder::evomap()` or inject into `ExecutionConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvoMapConfig {
    /// Agent node ID on the EvoMap network
    pub node_id: String,
    /// API key / node secret for authentication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Hub URL (default: `https://evomap.ai`)
    #[serde(default = "default_hub_url")]
    pub hub_url: String,
    /// Model identifier reported during registration
    #[serde(default = "default_model")]
    pub model: String,
    /// Whether to auto-report execution outcomes (default: true)
    #[serde(default = "default_true")]
    pub auto_report: bool,
}

fn default_hub_url() -> String { "https://evomap.ai".to_string() }
fn default_model() -> String { "riceprompt-engine".to_string() }
fn default_true() -> bool { true }

impl Default for EvoMapConfig {
    fn default() -> Self {
        Self {
            node_id: String::new(),
            api_key: None,
            hub_url: default_hub_url(),
            model: default_model(),
            auto_report: true,
        }
    }
}

impl EvoMapConfig {
    /// Create a new config with the given node ID
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            ..Default::default()
        }
    }

    /// Set the API key
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Set the hub URL
    pub fn with_hub_url(mut self, url: impl Into<String>) -> Self {
        self.hub_url = url.into();
        self
    }

    /// Set the model identifier
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Enable/disable auto-reporting of execution outcomes
    pub fn with_auto_report(mut self, enabled: bool) -> Self {
        self.auto_report = enabled;
        self
    }
}

// ============================================================================
// A2A Client
// ============================================================================

/// Lightweight EvoMap A2A protocol client.
///
/// All methods are async and use `reqwest` for HTTP.
pub struct EvoMapClient {
    client: Client,
    config: EvoMapConfig,
}

impl EvoMapClient {
    /// Create a new client from config.
    pub fn new(config: EvoMapConfig) -> Self {
        let mut builder = Client::builder();

        if let Some(ref key) = config.api_key {
            let mut headers = reqwest::header::HeaderMap::new();
            if let Ok(val) = reqwest::header::HeaderValue::from_str(
                &format!("Bearer {}", key)
            ) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
            builder = builder.default_headers(headers);
        }

        Self {
            client: builder.build().unwrap_or_default(),
            config,
        }
    }

    /// Generate a unique message ID.
    fn make_message_id() -> String {
        format!("msg_{}", uuid::Uuid::new_v4().to_string().replace('-', "")[..12].to_string())
    }

    /// Build a GEP protocol envelope.
    fn envelope(&self, message_type: &str, payload: Value) -> Value {
        json!({
            "protocol": "gep-a2a",
            "message_type": message_type,
            "message_id": Self::make_message_id(),
            "sender_id": self.config.node_id,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "payload": payload,
        })
    }

    /// Send a POST request to the Hub.
    async fn post(&self, path: &str, body: Value) -> Result<Value, EngineError> {
        let url = format!("{}{}", self.config.hub_url.trim_end_matches('/'), path);

        debug!(url = %url, message_type = %body["message_type"], "EvoMap A2A request");

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| EngineError::NodeExecutionFailed(
                format!("EvoMap request failed: {}", e)
            ))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(EngineError::NodeExecutionFailed(
                format!("EvoMap API {}: {}", status, body_text)
            ));
        }

        response.json::<Value>().await
            .map_err(|e| EngineError::NodeExecutionFailed(
                format!("EvoMap response parse error: {}", e)
            ))
    }

    // ── Public API ─────────────────────────────────────────────────────

    /// Register / heartbeat — `POST /a2a/hello`
    pub async fn hello(&self) -> Result<Value, EngineError> {
        let body = self.envelope("hello", json!({
            "capabilities": {
                "supported_types": ["Gene", "Capsule"]
            },
            "model": self.config.model,
        }));

        let response = self.post("/a2a/hello", body).await?;

        info!(
            node_id = %self.config.node_id,
            status = %response["payload"]["status"],
            "EvoMap hello acknowledged"
        );

        Ok(response)
    }

    /// Record an execution outcome — `POST /a2a/memory/record`
    ///
    /// Feeds the evolution memory with signal-gene-outcome mappings.
    pub async fn record_outcome(
        &self,
        workflow_name: &str,
        status: &str,  // "success" | "failed"
        score: f64,
        summary: &str,
        signals: &[&str],
    ) -> Result<Value, EngineError> {
        let body = self.envelope("record_outcome", json!({
            "geneId": "workflow_execution",
            "signals": signals,
            "status": status,
            "score": score,
            "summary": summary,
            "metadata": {
                "workflow": workflow_name,
                "engine": "rp-engine",
            }
        }));

        let response = self.post("/a2a/memory/record", body).await?;

        debug!(
            workflow = %workflow_name,
            status = %status,
            score = score,
            "EvoMap outcome recorded"
        );

        Ok(response)
    }

    /// Recall past experience — `POST /a2a/memory/recall`
    ///
    /// Query evolution memory for relevant prior outcomes.
    pub async fn recall(
        &self,
        query: &str,
        signals: &[&str],
        limit: u32,
    ) -> Result<Value, EngineError> {
        let body = self.envelope("recall", json!({
            "query": query,
            "signals": signals,
            "limit": limit,
        }));

        self.post("/a2a/memory/recall", body).await
    }

    /// Publish an evolution asset (Gene or Capsule) — `POST /a2a/publish`
    pub async fn publish(&self, bundle: Value) -> Result<Value, EngineError> {
        let body = self.envelope("publish", json!({
            "bundle": bundle,
        }));

        self.post("/a2a/publish", body).await
    }
}

// ============================================================================
// Post-execution hook
// ============================================================================

/// Report workflow execution outcome to EvoMap Hub.
///
/// Called automatically after `execute_workflow()` when `evomap` config is present
/// and `auto_report` is true. Runs as a background task (fire-and-forget).
pub async fn report_execution_outcome(
    config: &EvoMapConfig,
    workflow_name: &str,
    status: &str,
    duration_ms: u64,
    error_msg: Option<&str>,
) {
    let client = EvoMapClient::new(config.clone());

    let score = if status == "success" { 1.0 } else { 0.0 };
    let summary = match error_msg {
        Some(err) => format!("Workflow '{}' failed: {}", workflow_name, err),
        None => format!("Workflow '{}' completed in {}ms", workflow_name, duration_ms),
    };

    match client.record_outcome(
        workflow_name,
        status,
        score,
        &summary,
        &["workflow_execution", "rp-engine"],
    ).await {
        Ok(_) => debug!(workflow = %workflow_name, "EvoMap outcome reported"),
        Err(e) => warn!(workflow = %workflow_name, error = %e, "Failed to report EvoMap outcome (non-fatal)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evomap_config_defaults() {
        let config = EvoMapConfig::new("test-node");
        assert_eq!(config.node_id, "test-node");
        assert_eq!(config.hub_url, "https://evomap.ai");
        assert_eq!(config.model, "riceprompt-engine");
        assert!(config.auto_report);
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_evomap_config_builder() {
        let config = EvoMapConfig::new("my-agent")
            .with_api_key("secret123")
            .with_hub_url("https://custom-hub.example.com")
            .with_model("gpt-4o")
            .with_auto_report(false);

        assert_eq!(config.node_id, "my-agent");
        assert_eq!(config.api_key.unwrap(), "secret123");
        assert_eq!(config.hub_url, "https://custom-hub.example.com");
        assert_eq!(config.model, "gpt-4o");
        assert!(!config.auto_report);
    }

    #[test]
    fn test_message_id_generation() {
        let id = EvoMapClient::make_message_id();
        assert!(id.starts_with("msg_"));
        assert!(id.len() > 4);
    }
}
