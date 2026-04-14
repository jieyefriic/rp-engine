//! Data connector implementations for various data sources

pub mod s3;
pub mod postgresql;
pub mod mysql;
pub mod mongodb;
pub mod qdrant;
pub mod redis;

use std::collections::HashMap;
use serde_json::Value;

use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;

/// Unified context for all connector executions.
///
/// This provides all connectors with consistent access to:
/// - Execution state (for reading node outputs)
/// - Execution config (for embedding providers, pools, data sources, etc.)
/// - Pre-resolved variables (for {paramName} placeholder replacement)
pub struct ConnectorContext<'a> {
    /// Execution state (read node outputs)
    pub state: &'a ExecutionState,
    /// Execution config (embedding providers, pools, etc.)
    pub config: &'a ExecutionConfig,
    /// Pre-resolved variables ({paramName} -> resolved Value)
    pub variables: &'a HashMap<String, Value>,
    /// Data source ID (for looking up connection pools)
    pub data_source_id: Option<&'a str>,
}

/// Helper function to resolve {{variable}} templates in a string
///
/// **DEPRECATED**: This legacy syntax (`{{node.field}}`) performs string interpolation
/// which is unsafe for SQL (injection risk) and loses type information.
/// Use `variables` + `{paramName}` parameterized binding instead.
/// This function will be removed in a future version.
pub fn resolve_template_vars(template: &str, state: &ExecutionState) -> String {
    let re = regex_lite::Regex::new(r"\{\{([^}]+)\}\}").unwrap();

    let result = re.replace_all(template, |caps: &regex_lite::Captures| {
        let path = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        tracing::warn!(
            path = %path,
            template = %template,
            "DEPRECATED: Using legacy {{{{}}}} variable syntax. \
             Migrate to 'variables' + '{{paramName}}' parameterized binding. \
             Legacy syntax will be removed in a future version."
        );
        state.read(path)
            .map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            })
            .unwrap_or_else(|| format!("{{{{{}}}}}", path))
    }).to_string();

    result
}

/// Helper to get a required string field from connection config
pub fn get_required_string(
    connection: &HashMap<String, Value>,
    field: &str,
    state: &ExecutionState,
) -> Result<String, crate::error::EngineError> {
    connection.get(field)
        .and_then(|v| v.as_str())
        .map(|s| resolve_template_vars(s, state))
        .ok_or_else(|| crate::error::EngineError::MissingField(field.to_string()))
}

/// Helper to get an optional string field from connection config
pub fn get_optional_string(
    connection: &HashMap<String, Value>,
    field: &str,
    state: &ExecutionState,
) -> Option<String> {
    connection.get(field)
        .and_then(|v| v.as_str())
        .map(|s| resolve_template_vars(s, state))
}

/// Helper to get an optional u64 field
pub fn get_optional_u64(connection: &HashMap<String, Value>, field: &str) -> Option<u64> {
    connection.get(field).and_then(|v| v.as_u64())
}

/// Helper to get an optional bool field
pub fn get_optional_bool(connection: &HashMap<String, Value>, field: &str) -> Option<bool> {
    connection.get(field).and_then(|v| v.as_bool())
}

/// Validate a SQL identifier (table name, column name) to prevent injection.
/// Only allows alphanumeric characters, underscores, and dots (for schema.table).
/// Returns the sanitized identifier or an error.
pub fn validate_sql_identifier(name: &str) -> Result<String, crate::error::EngineError> {
    if name.is_empty() {
        return Err(crate::error::EngineError::DatabaseError(
            "Empty SQL identifier".to_string()
        ));
    }
    // Allow: letters, digits, underscore, dot (schema.table)
    // Reject everything else (quotes, semicolons, spaces, etc.)
    if name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
        Ok(name.to_string())
    } else {
        Err(crate::error::EngineError::DatabaseError(
            format!("Invalid SQL identifier '{}': only alphanumeric, underscore, and dot allowed", name)
        ))
    }
}

/// Validate a URL for SSRF protection.
/// Blocks private IP ranges, localhost, and non-HTTP schemes.
pub fn validate_url(url: &str) -> Result<(), crate::error::EngineError> {
    // Only allow http and https schemes
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(crate::error::EngineError::McpError(
            format!("Blocked URL '{}': only http/https schemes allowed", url)
        ));
    }

    // Extract host from URL (between :// and next / or : or end)
    let after_scheme = url.split("://").nth(1).unwrap_or("");
    let host = after_scheme.split(&['/', ':', '?'][..]).next().unwrap_or("");
    let host_lower = host.to_lowercase();

    // Block private/internal addresses
    let blocked = matches!(host_lower.as_str(),
        "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]"
        | "169.254.169.254"          // AWS/GCP/Azure metadata
        | "metadata.google.internal" // GCP metadata
    ) || host_lower.starts_with("10.")
      || host_lower.starts_with("192.168.")
      || (host_lower.starts_with("172.") && {
          // 172.16.0.0 - 172.31.255.255
          host_lower.split('.').nth(1)
              .and_then(|s| s.parse::<u8>().ok())
              .map(|n| (16..=31).contains(&n))
              .unwrap_or(false)
      });

    if blocked {
        return Err(crate::error::EngineError::McpError(
            format!("Blocked URL '{}': private/internal addresses not allowed", url)
        ));
    }

    Ok(())
}
