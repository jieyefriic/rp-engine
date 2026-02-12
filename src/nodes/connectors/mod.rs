//! Data connector implementations for various data sources

pub mod minio;
pub mod postgresql;
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
