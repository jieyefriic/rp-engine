//! MCP node executor — calls MCP servers via Streamable HTTP transport (JSON-RPC 2.0)
//!
//! Implements Scenario A: explicit tool call. The workflow author specifies
//! which MCP server and tool to call. Arguments come from variable resolution.

use std::collections::HashMap;
use std::time::Instant;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, info, warn, error};

use crate::schema::{YamlNode, McpAuth};
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::NodeExecutor;

// ─── MCP Client (Streamable HTTP JSON-RPC) ───────────────────────────

/// Lightweight MCP client for Streamable HTTP transport.
/// Handles initialize handshake and tool invocation.
pub(super) struct McpClient {
    client: Client,
    url: String,
    session_id: Option<String>,
    request_id: u64,
}

impl McpClient {
    /// Create a new MCP client (public for use by generate node dispatch)
    pub(super) fn new_pub(url: &str, auth: Option<&McpAuth>) -> Self {
        Self::new(url, auth)
    }

    /// Create a new MCP client with optional authentication.
    /// URL is validated for SSRF protection (blocks private IPs, non-HTTP schemes).
    fn new(url: &str, auth: Option<&McpAuth>) -> Self {
        let mut builder = Client::builder();

        // Set default auth headers if configured
        if let Some(auth) = auth {
            let mut headers = reqwest::header::HeaderMap::new();
            match auth.auth_type.as_str() {
                "bearer" => {
                    if let Ok(val) = reqwest::header::HeaderValue::from_str(
                        &format!("Bearer {}", auth.token)
                    ) {
                        headers.insert(reqwest::header::AUTHORIZATION, val);
                    }
                }
                "api_key" => {
                    if let Ok(val) = reqwest::header::HeaderValue::from_str(&auth.token) {
                        headers.insert("X-API-Key", val);
                    }
                }
                other => {
                    warn!(auth_type = %other, "Unknown MCP auth type, skipping");
                }
            }
            builder = builder.default_headers(headers);
        }

        Self {
            client: builder.build().unwrap_or_default(),
            url: url.to_string(),
            session_id: None,
            request_id: 0,
        }
    }

    /// Get the next request ID
    fn next_id(&mut self) -> u64 {
        self.request_id += 1;
        self.request_id
    }

    /// Send a JSON-RPC request and parse the response
    async fn rpc_call(&mut self, method: &str, params: Value) -> Result<Value, EngineError> {
        let id = self.next_id();
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });

        let mut request = self.client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body);

        // Include session ID if we have one
        if let Some(ref sid) = self.session_id {
            request = request.header("Mcp-Session-Id", sid);
        }

        debug!(method = %method, id = id, "MCP JSON-RPC request");

        let response = request.send().await
            .map_err(|e| EngineError::McpError(format!("HTTP request failed: {}", e)))?;

        // Capture session ID from response headers
        if let Some(sid) = response.headers().get("Mcp-Session-Id") {
            if let Ok(sid_str) = sid.to_str() {
                self.session_id = Some(sid_str.to_string());
            }
        }

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(EngineError::McpError(format!(
                "HTTP {}: {}", status, body_text
            )));
        }

        // Parse response — handle both plain JSON and SSE (text/event-stream) formats
        let content_type = response.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let rpc_response: Value = if content_type.contains("text/event-stream") {
            // SSE format: parse "data:" lines for JSON-RPC response
            let body = response.text().await
                .map_err(|e| EngineError::McpError(format!("Failed to read SSE body: {}", e)))?;

            parse_sse_json_rpc(&body)?
        } else {
            // Plain JSON response
            response.json().await
                .map_err(|e| EngineError::McpError(format!("Failed to parse JSON-RPC response: {}", e)))?
        };

        // Check for JSON-RPC error
        if let Some(err) = rpc_response.get("error") {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let message = err.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            return Err(EngineError::McpError(format!(
                "JSON-RPC error {}: {}", code, message
            )));
        }

        Ok(rpc_response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Initialize the MCP session (required before tool calls)
    pub(super) async fn initialize(&mut self) -> Result<Value, EngineError> {
        let params = json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "rp-engine",
                "version": "0.1.0"
            }
        });

        let result = self.rpc_call("initialize", params).await?;

        debug!(
            session_id = ?self.session_id,
            server_info = %serde_json::to_string(&result.get("serverInfo")).unwrap_or_default(),
            "MCP session initialized"
        );

        // Send initialized notification (no response expected, but some servers require it)
        if let Err(e) = self.rpc_call("notifications/initialized", json!({})).await {
            debug!(error = %e, "MCP initialized notification failed (non-fatal)");
        }

        Ok(result)
    }

    /// Discover available tools from the MCP server
    async fn list_tools(&mut self) -> Result<Vec<Value>, EngineError> {
        let result = self.rpc_call("tools/list", json!({})).await?;

        let tools = result.get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        debug!(tool_count = tools.len(), "MCP tools/list completed");
        Ok(tools)
    }

    /// Call a tool on the MCP server
    pub(super) async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, EngineError> {
        let params = json!({
            "name": name,
            "arguments": arguments,
        });

        let result = self.rpc_call("tools/call", params).await?;

        // Extract content from MCP tool result format
        // MCP returns: { "content": [{ "type": "text", "text": "..." }], "isError": false }
        if let Some(content_array) = result.get("content").and_then(|v| v.as_array()) {
            // Collect all text content parts
            let texts: Vec<&str> = content_array.iter()
                .filter_map(|part| {
                    if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                        part.get("text").and_then(|t| t.as_str())
                    } else {
                        None
                    }
                })
                .collect();

            if texts.len() == 1 {
                // Try to parse as JSON first
                if let Ok(parsed) = serde_json::from_str::<Value>(texts[0]) {
                    return Ok(parsed);
                }
                return Ok(Value::String(texts[0].to_string()));
            } else if !texts.is_empty() {
                return Ok(Value::String(texts.join("\n")));
            }
        }

        // Fallback: return raw result
        Ok(result)
    }
}

/// Parse SSE (Server-Sent Events) body to extract JSON-RPC response.
/// SSE format: lines starting with "data:" contain the JSON payload.
fn parse_sse_json_rpc(body: &str) -> Result<Value, EngineError> {
    for line in body.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() { continue; }
            match serde_json::from_str::<Value>(data) {
                Ok(val) => {
                    // Check if this is a JSON-RPC response (has "id" field)
                    if val.get("id").is_some() || val.get("result").is_some() || val.get("error").is_some() {
                        return Ok(val);
                    }
                }
                Err(_) => continue,
            }
        }
    }
    Err(EngineError::McpError("No JSON-RPC response found in SSE stream".to_string()))
}

// ─── MCP Tools Executor (Scenario B: tool discovery) ─────────────────

/// Executor for mcp_tools nodes — discovers tools from MCP server via tools/list.
/// Outputs tool definitions to state for downstream generate nodes to inject.
pub struct McpToolsExecutor;

#[async_trait]
impl NodeExecutor for McpToolsExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        _state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_id = &node.id;
        let node_config = &node.config;

        // 1. Get server reference
        let server_id = node_config.server.as_ref()
            .ok_or_else(|| EngineError::McpError(
                format!("Node '{}': mcp_tools requires 'server' field", node_id)
            ))?;

        // 2. Look up server definition
        let server_def = config.mcp_servers.get(server_id)
            .ok_or_else(|| EngineError::McpError(
                format!("Node '{}': MCP server '{}' not found", node_id, server_id)
            ))?;

        // SSRF validation
        super::connectors::validate_url(&server_def.url)?;

        info!(
            node = %node_id,
            server = %server_id,
            url = %server_def.url,
            "Discovering MCP tools"
        );

        // 3. Connect and list tools
        let mut client = McpClient::new(&server_def.url, server_def.auth.as_ref());
        client.initialize().await?;
        let tools = client.list_tools().await?;

        let tool_count = tools.len();
        info!(
            node = %node_id,
            server = %server_id,
            tool_count = tool_count,
            "MCP tools discovered"
        );

        // 4. Output to state
        let mut outputs = HashMap::new();
        outputs.insert("tools".to_string(), Value::Array(tools));
        outputs.insert("server".to_string(), Value::String(server_id.clone()));
        outputs.insert("_tool_count".to_string(), json!(tool_count));

        Ok(outputs)
    }
}

// ─── MCP Executor (Scenario A: explicit tool call) ───────────────────

/// Executor for MCP nodes (Scenario A: explicit tool call)
pub struct McpExecutor;

#[async_trait]
impl NodeExecutor for McpExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_config = &node.config;
        let node_id = &node.id;
        let start_time = Instant::now();

        // 1. Get server reference
        let server_id = node_config.server.as_ref()
            .ok_or_else(|| {
                error!(node = %node_id, "MCP node requires 'server' config field");
                EngineError::McpError(format!("Node '{}': missing 'server' field", node_id))
            })?;

        // 2. Look up server definition
        let server_def = config.mcp_servers.get(server_id)
            .ok_or_else(|| {
                error!(node = %node_id, server = %server_id, "MCP server not found");
                EngineError::McpError(format!(
                    "Node '{}': MCP server '{}' not found. Register via Engine::builder().mcp_server() or workflow mcp_servers section.",
                    node_id, server_id
                ))
            })?;

        // 3. Get tool name
        let tool_name = node_config.tool.as_ref()
            .ok_or_else(|| {
                error!(node = %node_id, "MCP node requires 'tool' config field");
                EngineError::McpError(format!("Node '{}': missing 'tool' field", node_id))
            })?;

        // 4. Resolve variables as tool arguments
        let variables = super::resolve_variables(&node_config.variables, state);
        let arguments: Value = serde_json::to_value(&variables)
            .unwrap_or(json!({}));

        // SSRF validation
        super::connectors::validate_url(&server_def.url)?;

        info!(
            node = %node_id,
            server = %server_id,
            tool = %tool_name,
            url = %server_def.url,
            "Executing MCP tool call"
        );

        // 5. Create client, initialize, call tool
        let mut client = McpClient::new(&server_def.url, server_def.auth.as_ref());

        // Initialize with one retry
        match client.initialize().await {
            Ok(_) => {}
            Err(e) => {
                warn!(node = %node_id, error = %e, "MCP initialize failed, retrying once");
                client = McpClient::new(&server_def.url, server_def.auth.as_ref());
                client.initialize().await.map_err(|e2| {
                    error!(node = %node_id, error = %e2, "MCP initialize retry failed");
                    EngineError::McpError(format!("Node '{}': MCP initialize failed after retry: {}", node_id, e2))
                })?;
            }
        }

        let result = client.call_tool(tool_name, arguments).await;

        let timing_ms = start_time.elapsed().as_millis() as u64;

        // 6. Build outputs
        let mut outputs = HashMap::new();
        outputs.insert("_timing_ms".to_string(), json!(timing_ms));

        match result {
            Ok(value) => {
                info!(
                    node = %node_id,
                    tool = %tool_name,
                    timing_ms = timing_ms,
                    "MCP tool call succeeded"
                );

                // If result is an object, flatten fields to top level
                if let Value::Object(map) = &value {
                    for (k, v) in map {
                        outputs.insert(k.clone(), v.clone());
                    }
                }
                outputs.insert("result".to_string(), value);
            }
            Err(e) => {
                warn!(
                    node = %node_id,
                    tool = %tool_name,
                    error = %e,
                    "MCP tool call failed"
                );
                outputs.insert("_error".to_string(), Value::Bool(true));
                outputs.insert("_error_message".to_string(), Value::String(e.to_string()));
                outputs.insert("result".to_string(), Value::Null);
            }
        }

        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_client_creation() {
        let client = McpClient::new("https://example.com/mcp", None);
        assert_eq!(client.url, "https://example.com/mcp");
        assert!(client.session_id.is_none());
        assert_eq!(client.request_id, 0);
    }

    #[test]
    fn test_mcp_client_with_bearer_auth() {
        let auth = McpAuth {
            auth_type: "bearer".to_string(),
            token: "test-token".to_string(),
        };
        let client = McpClient::new("https://example.com/mcp", Some(&auth));
        assert_eq!(client.url, "https://example.com/mcp");
    }

    #[test]
    fn test_mcp_client_request_id_increment() {
        let mut client = McpClient::new("https://example.com/mcp", None);
        assert_eq!(client.next_id(), 1);
        assert_eq!(client.next_id(), 2);
        assert_eq!(client.next_id(), 3);
    }
}
