//! Redis data connector implementation
//!
//! Simplified operations: get, set, del, publish, append_history
//!
//! Supports both legacy `key` field and new `key_template` with parameter bindings.

use std::collections::HashMap;
use redis::AsyncCommands;
use serde_json::Value;
use tracing::{debug, error};

use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, ConnectorContext};

/// Resolve {paramName} patterns in a template using bindings
///
/// For example, with:
/// - template: "user:{userId}"
/// - bindings: {"userId": "start.user_id"}
/// - state contains: {"start.user_id": "123"}
///
/// Returns: "user:123"
fn resolve_param_template(
    template: &str,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> String {
    let re = regex_lite::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();

    re.replace_all(template, |caps: &regex_lite::Captures| {
        let param_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");

        // Look up the binding for this parameter
        if let Some(state_path) = bindings.get(param_name) {
            // Resolve the state path to get the actual value
            state.read(state_path)
                .map(|v| match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                })
                .unwrap_or_else(|| format!("{{{}}}", param_name))
        } else {
            // No binding found, leave the placeholder as-is
            format!("{{{}}}", param_name)
        }
    }).to_string()
}

/// Get the Redis key from operation config
///
/// Supports both:
/// - Legacy: "key" field with {{node.field}} syntax
/// - New: "key_template" field with {paramName} syntax + bindings
fn get_key(
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    // Try new key_template first
    if let Some(template) = operation.get("key_template").and_then(|v| v.as_str()) {
        return Ok(resolve_param_template(template, bindings, state));
    }

    // Fall back to legacy key
    get_required_string(operation, "key", state)
}

/// Get the Redis value from operation config (for SET operations)
///
/// Supports both:
/// - Legacy: "value" field with {{node.field}} syntax
/// - New: "value_template" field with {paramName} syntax + bindings
fn get_value(
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    // Try new value_template first
    if let Some(template) = operation.get("value_template").and_then(|v| v.as_str()) {
        return Ok(resolve_param_template(template, bindings, state));
    }

    // Fall back to legacy value
    get_required_string(operation, "value", state)
}

/// Unified Redis execute function
///
/// Takes data source config, operation config, and connector context.
/// Uses bindings from context for parameter resolution.
pub async fn execute(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    ctx: &ConnectorContext<'_>,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get connection URL from data source config
    let url = get_connection_url(ds_config, ctx.state)?;

    // Get operation type
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!(operation_config = ?operation, "Redis operation type not specified");
            EngineError::MissingField("operation".to_string())
        })?;

    debug!(
        operation = %op_type,
        "Starting Redis operation"
    );

    // Create Redis client
    let client = redis::Client::open(url.as_str())
        .map_err(|e| {
            error!(
                error = %e,
                url = %url,
                "Failed to create Redis client"
            );
            EngineError::RedisError(format!("Failed to create Redis client: {}", e))
        })?;

    let mut conn = client.get_multiplexed_async_connection().await
        .map_err(|e| {
            error!(
                error = %e,
                url = %url,
                "Failed to connect to Redis"
            );
            EngineError::RedisError(format!("Failed to connect to Redis: {}", e))
        })?;

    let result = match op_type {
        "get" => execute_get_v2(&mut conn, operation, ctx.bindings, ctx.state).await,
        "set" => execute_set_v2(&mut conn, operation, ctx.bindings, ctx.state).await,
        "del" => execute_del_v2(&mut conn, operation, ctx.bindings, ctx.state).await,
        "publish" => execute_publish_v2(&mut conn, operation, ctx.bindings, ctx.state).await,
        "append_history" => execute_append_history_v2(&mut conn, operation, ctx.bindings, ctx.state).await,
        _ => {
            error!(operation = %op_type, "Unknown Redis operation type");
            Err(EngineError::InvalidOperation(format!("Unknown Redis operation: {}", op_type)))
        }
    };

    result.map_err(|e| {
        let key = get_key(operation, ctx.bindings, ctx.state).unwrap_or_else(|_| "(unknown)".to_string());
        error!(
            operation = %op_type,
            key = %key,
            error = %e,
            "Redis operation failed"
        );
        e
    })
}

/// Build Redis connection URL from config
fn get_connection_url(
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    // Try direct URL first
    if let Some(url) = get_optional_string(connection, "url", state) {
        return Ok(url);
    }

    // Build from components
    let host = get_optional_string(connection, "host", state)
        .unwrap_or_else(|| "localhost".to_string());
    let port = get_optional_u64(connection, "port").unwrap_or(6379);
    let password = get_optional_string(connection, "password", state);
    let db = get_optional_u64(connection, "db").unwrap_or(0);

    let url = if let Some(pass) = password {
        format!("redis://:{}@{}:{}/{}", pass, host, port, db)
    } else {
        format!("redis://{}:{}/{}", host, port, db)
    };

    Ok(url)
}

// ============================================
// Operations with bindings support
// ============================================

/// Execute GET operation (V2 with bindings)
async fn execute_get_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, bindings, state)?;

    let result: Option<String> = conn.get(&key).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();

    match result {
        Some(value) => {
            // Try to parse as JSON
            let parsed_value = serde_json::from_str::<Value>(&value)
                .unwrap_or(Value::String(value.clone()));

            // Check if output_schema is defined
            if let Some(schema) = operation.get("output_schema") {
                if let Value::Array(fields) = schema {
                    // Extract fields according to schema
                    if let Value::Object(obj) = &parsed_value {
                        for field_def in fields {
                            if let Some(field_name) = field_def.get("name").and_then(|n| n.as_str()) {
                                if let Some(field_value) = obj.get(field_name) {
                                    outputs.insert(field_name.to_string(), field_value.clone());
                                }
                            }
                        }
                    }
                }
            }

            // Always include raw value and exists flag
            outputs.insert("value".to_string(), parsed_value);
            outputs.insert("exists".to_string(), Value::Bool(true));
        }
        None => {
            outputs.insert("value".to_string(), Value::Null);
            outputs.insert("exists".to_string(), Value::Bool(false));
        }
    }

    Ok(outputs)
}

/// Execute SET operation (V2 with bindings)
async fn execute_set_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, bindings, state)?;
    let value = get_value(operation, bindings, state)?;

    // Get optional TTL
    let ttl = get_optional_u64(operation, "ttl");

    let result: bool = if let Some(seconds) = ttl {
        conn.set_ex(&key, &value, seconds)
            .await
            .map(|_: ()| true)
            .map_err(|e| EngineError::RedisError(e.to_string()))?
    } else {
        conn.set(&key, &value)
            .await
            .map(|_: ()| true)
            .map_err(|e| EngineError::RedisError(e.to_string()))?
    };

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(result));
    Ok(outputs)
}

/// Execute DEL operation (V2 with bindings)
async fn execute_del_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, bindings, state)?;

    let deleted: i64 = conn.del(&key)
        .await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("deleted".to_string(), Value::Number(deleted.into()));
    outputs.insert("ok".to_string(), Value::Bool(deleted > 0));
    Ok(outputs)
}

/// Execute PUBLISH operation (V2 with bindings)
/// Uses key_template as the channel and value_template as the message
async fn execute_publish_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let channel = get_key(operation, bindings, state)?; // key_template is the channel
    let message = get_value(operation, bindings, state)?; // value_template is the message

    let subscribers: i64 = conn.publish(&channel, &message)
        .await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("subscribers".to_string(), Value::Number(subscribers.into()));
    outputs.insert("ok".to_string(), Value::Bool(true));
    Ok(outputs)
}

/// Execute APPEND_HISTORY operation (V2 with bindings)
/// Appends messages to chat_history array in a JSON object
///
/// Expected operation config:
/// - key_template: Redis key
/// - user_message: User message to append (resolved from bindings)
/// - assistant_message: Assistant message to append (resolved from bindings)
/// - message_type: Type of user message (text/voice/image)
/// - ttl: Optional TTL in seconds
async fn execute_append_history_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, bindings, state)?;

    // Get message content from bindings
    let user_message = if let Some(path) = bindings.get("userMessage") {
        state.read(path).map(|v| match v {
            Value::String(s) => s,
            other => other.to_string(),
        })
    } else {
        None
    };

    let assistant_message = if let Some(path) = bindings.get("assistantMessage") {
        state.read(path).map(|v| match v {
            Value::String(s) => s,
            other => other.to_string(),
        })
    } else {
        None
    };

    let message_type = if let Some(path) = bindings.get("messageType") {
        state.read(path).map(|v| match v {
            Value::String(s) => s,
            other => other.to_string(),
        }).unwrap_or_else(|| "text".to_string())
    } else {
        "text".to_string()
    };

    // Read existing value
    let existing: Option<String> = conn.get(&key).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    // Parse or create new structure
    let mut data: serde_json::Map<String, Value> = match existing {
        Some(json_str) => {
            serde_json::from_str(&json_str).unwrap_or_else(|_| serde_json::Map::new())
        }
        None => serde_json::Map::new(),
    };

    // Get or create chat_history array
    let chat_history = data.entry("chat_history".to_string())
        .or_insert_with(|| Value::Array(vec![]));

    if let Value::Array(history) = chat_history {
        // Append user message if present
        if let Some(content) = user_message {
            history.push(serde_json::json!({
                "role": "user",
                "content": content,
                "type": message_type
            }));
        }

        // Append assistant message if present
        if let Some(content) = assistant_message {
            history.push(serde_json::json!({
                "role": "assistant",
                "content": content
            }));
        }
    }

    // Serialize and save
    let json_str = serde_json::to_string(&data)
        .map_err(|e| EngineError::RedisError(format!("JSON serialize error: {}", e)))?;

    // Get optional TTL
    let ttl = get_optional_u64(operation, "ttl");

    if let Some(seconds) = ttl {
        let _: () = conn.set_ex(&key, &json_str, seconds).await
            .map_err(|e| EngineError::RedisError(e.to_string()))?;
    } else {
        let _: () = conn.set(&key, &json_str).await
            .map_err(|e| EngineError::RedisError(e.to_string()))?;
    }

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(true));
    Ok(outputs)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_connection_url_from_url() {
        let mut connection = HashMap::new();
        connection.insert("url".to_string(), Value::String("redis://localhost:6379".to_string()));

        let state = ExecutionState::new(HashMap::new());
        let url = get_connection_url(&connection, &state).unwrap();

        assert_eq!(url, "redis://localhost:6379");
    }

    #[test]
    fn test_get_connection_url_from_components() {
        let mut connection = HashMap::new();
        connection.insert("host".to_string(), Value::String("myhost".to_string()));
        connection.insert("port".to_string(), Value::Number(6380.into()));
        connection.insert("password".to_string(), Value::String("secret".to_string()));
        connection.insert("db".to_string(), Value::Number(1.into()));

        let state = ExecutionState::new(HashMap::new());
        let url = get_connection_url(&connection, &state).unwrap();

        assert_eq!(url, "redis://:secret@myhost:6380/1");
    }

    #[test]
    fn test_resolve_param_template() {
        // Setup state with test data
        let mut state = ExecutionState::new(HashMap::new());
        let mut start_outputs = HashMap::new();
        start_outputs.insert("user_id".to_string(), Value::String("123".to_string()));
        start_outputs.insert("region".to_string(), Value::String("us-west".to_string()));
        state.write("start", start_outputs);

        // Setup bindings
        let mut bindings = HashMap::new();
        bindings.insert("userId".to_string(), "start.user_id".to_string());
        bindings.insert("region".to_string(), "start.region".to_string());

        // Test single parameter
        let result = resolve_param_template("user:{userId}", &bindings, &state);
        assert_eq!(result, "user:123");

        // Test multiple parameters
        let result = resolve_param_template("cache:{region}:{userId}", &bindings, &state);
        assert_eq!(result, "cache:us-west:123");

        // Test no parameters
        let result = resolve_param_template("static_key", &bindings, &state);
        assert_eq!(result, "static_key");

        // Test unbound parameter (should keep placeholder)
        let result = resolve_param_template("user:{unknownParam}", &bindings, &state);
        assert_eq!(result, "user:{unknownParam}");
    }

    #[test]
    fn test_get_key_with_template() {
        // Setup state with test data
        let mut state = ExecutionState::new(HashMap::new());
        let mut start_outputs = HashMap::new();
        start_outputs.insert("user_id".to_string(), Value::String("456".to_string()));
        state.write("start", start_outputs);

        // Setup bindings
        let mut bindings = HashMap::new();
        bindings.insert("userId".to_string(), "start.user_id".to_string());

        // Test with key_template
        let mut operation = HashMap::new();
        operation.insert("key_template".to_string(), Value::String("user:{userId}".to_string()));

        let result = get_key(&operation, &bindings, &state).unwrap();
        assert_eq!(result, "user:456");
    }

    #[test]
    fn test_get_key_legacy() {
        // Setup state with test data
        let mut state = ExecutionState::new(HashMap::new());
        let mut start_outputs = HashMap::new();
        start_outputs.insert("user_id".to_string(), Value::String("789".to_string()));
        state.write("start", start_outputs);

        // Empty bindings (legacy mode)
        let bindings = HashMap::new();

        // Test with legacy key using {{}} syntax
        let mut operation = HashMap::new();
        operation.insert("key".to_string(), Value::String("user:{{start.user_id}}".to_string()));

        let result = get_key(&operation, &bindings, &state).unwrap();
        assert_eq!(result, "user:789");
    }
}
