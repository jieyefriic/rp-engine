//! Data connector implementations for various data sources

pub mod minio;
pub mod postgresql;
pub mod qdrant;
pub mod redis;

use std::collections::HashMap;
use serde_json::Value;

use crate::state::ExecutionState;

/// Helper function to resolve {{variable}} templates in a string
pub fn resolve_template_vars(template: &str, state: &ExecutionState) -> String {
    let re = regex_lite::Regex::new(r"\{\{([^}]+)\}\}").unwrap();

    re.replace_all(template, |caps: &regex_lite::Captures| {
        let path = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        state.read(path)
            .map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            })
            .unwrap_or_else(|| format!("{{{{{}}}}}", path))
    }).to_string()
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

/// Helper to get an optional i64 field
pub fn get_optional_i64(connection: &HashMap<String, Value>, field: &str) -> Option<i64> {
    connection.get(field).and_then(|v| v.as_i64())
}

/// Helper to get an optional bool field
pub fn get_optional_bool(connection: &HashMap<String, Value>, field: &str) -> Option<bool> {
    connection.get(field).and_then(|v| v.as_bool())
}
