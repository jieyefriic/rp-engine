//! Redis data connector implementation
//!
//! Simplified operations: get, set, del, publish, append_history
//!
//! Uses `key_template` / `value_template` with pre-resolved variables (recommended).
//! Legacy `key` / `value` fields with `{{node.field}}` syntax are deprecated.

use std::collections::HashMap;
use redis::AsyncCommands;
use serde_json::Value;
use tracing::{debug, error};

use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, ConnectorContext};

/// Resolve {paramName} patterns in a template using pre-resolved variables
///
/// For example, with:
/// - template: "user:{userId}"
/// - variables: {"userId": Value::String("123")}
///
/// Returns: "user:123"
fn resolve_param_template(
    template: &str,
    variables: &HashMap<String, Value>,
) -> String {
    let re = regex_lite::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();

    re.replace_all(template, |caps: &regex_lite::Captures| {
        let param_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");

        // Look up the pre-resolved value for this parameter
        if let Some(value) = variables.get(param_name) {
            match value {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            }
        } else {
            // No variable found, leave the placeholder as-is
            format!("{{{}}}", param_name)
        }
    }).to_string()
}

/// Get the Redis key from operation config
///
/// Preferred: `key_template` with `{paramName}` syntax + pre-resolved variables
/// Deprecated: `key` field with `{{node.field}}` syntax
fn get_key(
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    // Preferred: key_template with {paramName} variables
    if let Some(template) = operation.get("key_template").and_then(|v| v.as_str()) {
        return Ok(resolve_param_template(template, variables));
    }

    // Deprecated fallback: legacy key with {{node.field}}
    tracing::warn!("DEPRECATED: Redis 'key' field uses legacy {{{{}}}} syntax. Migrate to 'key_template' + 'variables'.");
    get_required_string(operation, "key", state)
}

/// Get the Redis value from operation config (for SET operations)
///
/// Preferred: `value_template` with `{paramName}` syntax + pre-resolved variables
/// Deprecated: `value` field with `{{node.field}}` syntax
fn get_value(
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    // Preferred: value_template with {paramName} variables
    if let Some(template) = operation.get("value_template").and_then(|v| v.as_str()) {
        return Ok(resolve_param_template(template, variables));
    }

    // Deprecated fallback: legacy value with {{node.field}}
    tracing::warn!("DEPRECATED: Redis 'value' field uses legacy {{{{}}}} syntax. Migrate to 'value_template' + 'variables'.");
    get_required_string(operation, "value", state)
}

/// Unified Redis execute function
///
/// Takes data source config, operation config, and connector context.
/// Uses pre-resolved variables from context for parameter resolution.
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
        "get" => execute_get_v2(&mut conn, operation, ctx.variables, ctx.state).await,
        "set" => execute_set_v2(&mut conn, operation, ctx.variables, ctx.state).await,
        "del" => execute_del_v2(&mut conn, operation, ctx.variables, ctx.state).await,
        "publish" => execute_publish_v2(&mut conn, operation, ctx.variables, ctx.state).await,
        "append_history" => execute_append_history_v2(&mut conn, operation, ctx.variables).await,
        _ => {
            error!(operation = %op_type, "Unknown Redis operation type");
            Err(EngineError::InvalidOperation(format!("Unknown Redis operation: {}", op_type)))
        }
    };

    result.map_err(|e| {
        let key = get_key(operation, ctx.variables, ctx.state).unwrap_or_else(|_| "(unknown)".to_string());
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
// Operations with variables support
// ============================================

/// Execute GET operation (V2 with pre-resolved variables)
async fn execute_get_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables, state)?;

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
                            if let (Some(field_name), Some(field_type)) = (
                                field_def.get("name").and_then(|n| n.as_str()),
                                field_def.get("type").and_then(|t| t.as_str()),
                            ) {
                                if let Some(field_value) = obj.get(field_name) {
                                    // Field exists in Redis data
                                    outputs.insert(field_name.to_string(), field_value.clone());
                                } else {
                                    // Field defined in schema but not in Redis data - return empty value
                                    let empty_value = match field_type {
                                        "array" => Value::Array(vec![]),
                                        "string" => Value::String(String::new()),
                                        "number" => Value::Number(0.into()),
                                        "boolean" => Value::Bool(false),
                                        "object" => Value::Object(serde_json::Map::new()),
                                        _ => Value::Null,
                                    };
                                    outputs.insert(field_name.to_string(), empty_value);
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

            // TODO: Optimize Redis connector to support partial field reads and lazy loading
            // For now, return empty values based on output_schema so downstream nodes can execute
            if let Some(schema) = operation.get("output_schema") {
                if let Value::Array(fields) = schema {
                    for field_def in fields {
                        if let (Some(field_name), Some(field_type)) = (
                            field_def.get("name").and_then(|n| n.as_str()),
                            field_def.get("type").and_then(|t| t.as_str()),
                        ) {
                            let empty_value = match field_type {
                                "array" => Value::Array(vec![]),
                                "string" => Value::String(String::new()),
                                "number" => Value::Number(0.into()),
                                "boolean" => Value::Bool(false),
                                "object" => Value::Object(serde_json::Map::new()),
                                _ => Value::Null,
                            };
                            outputs.insert(field_name.to_string(), empty_value);
                        }
                    }
                }
            }
        }
    }

    Ok(outputs)
}

/// Execute SET operation (V2 with pre-resolved variables)
async fn execute_set_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables, state)?;
    let value = get_value(operation, variables, state)?;

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

/// Execute DEL operation (V2 with pre-resolved variables)
async fn execute_del_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables, state)?;

    let deleted: i64 = conn.del(&key)
        .await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("deleted".to_string(), Value::Number(deleted.into()));
    outputs.insert("ok".to_string(), Value::Bool(deleted > 0));
    Ok(outputs)
}

/// Execute PUBLISH operation (V2 with pre-resolved variables)
/// Uses key_template as the channel and value_template as the message
async fn execute_publish_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let channel = get_key(operation, variables, state)?; // key_template is the channel
    let message = get_value(operation, variables, state)?; // value_template is the message

    let subscribers: i64 = conn.publish(&channel, &message)
        .await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("subscribers".to_string(), Value::Number(subscribers.into()));
    outputs.insert("ok".to_string(), Value::Bool(true));
    Ok(outputs)
}

/// Execute APPEND_HISTORY operation (V2 with pre-resolved variables)
/// Appends messages to chat_history array in a JSON object
///
/// Expected variables:
/// - userId/characterId: Used in key_template
/// - userMessage: User message to append
/// - assistantMessage: Assistant message to append
/// - messageType: Type of user message (text/voice/image)
///
/// Expected operation config:
/// - key_template: Redis key with {paramName} placeholders
/// - ttl: Optional TTL in seconds
async fn execute_append_history_v2(
    conn: &mut redis::aio::MultiplexedConnection,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get key from key_template + variables, no state needed for legacy fallback
    let key = if let Some(template) = operation.get("key_template").and_then(|v| v.as_str()) {
        resolve_param_template(template, variables)
    } else {
        return Err(EngineError::MissingField("key_template".to_string()));
    };

    // Get message content from pre-resolved variables
    let user_message = variables.get("userMessage").and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Null => None,
        _ => None,
    });

    let assistant_message = variables.get("assistantMessage").and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Null => None,
        _ => None,
    });

    let message_type = variables.get("messageType")
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "text".to_string());

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
        // Setup pre-resolved variables
        let mut variables = HashMap::new();
        variables.insert("userId".to_string(), Value::String("123".to_string()));
        variables.insert("region".to_string(), Value::String("us-west".to_string()));

        // Test single parameter
        let result = resolve_param_template("user:{userId}", &variables);
        assert_eq!(result, "user:123");

        // Test multiple parameters
        let result = resolve_param_template("cache:{region}:{userId}", &variables);
        assert_eq!(result, "cache:us-west:123");

        // Test no parameters
        let result = resolve_param_template("static_key", &variables);
        assert_eq!(result, "static_key");

        // Test unbound parameter (should keep placeholder)
        let result = resolve_param_template("user:{unknownParam}", &variables);
        assert_eq!(result, "user:{unknownParam}");
    }

    #[test]
    fn test_get_key_with_template() {
        let state = ExecutionState::new(HashMap::new());

        // Setup pre-resolved variables
        let mut variables = HashMap::new();
        variables.insert("userId".to_string(), Value::String("456".to_string()));

        // Test with key_template
        let mut operation = HashMap::new();
        operation.insert("key_template".to_string(), Value::String("user:{userId}".to_string()));

        let result = get_key(&operation, &variables, &state).unwrap();
        assert_eq!(result, "user:456");
    }

    #[test]
    fn test_get_key_legacy() {
        // Setup state with test data
        let mut state = ExecutionState::new(HashMap::new());
        let mut start_outputs = HashMap::new();
        start_outputs.insert("user_id".to_string(), Value::String("789".to_string()));
        state.write("start", start_outputs);

        // Empty variables (legacy mode uses state directly via {{}} syntax)
        let variables = HashMap::new();

        // Test with legacy key using {{}} syntax
        let mut operation = HashMap::new();
        operation.insert("key".to_string(), Value::String("user:{{start.user_id}}".to_string()));

        let result = get_key(&operation, &variables, &state).unwrap();
        assert_eq!(result, "user:789");
    }
}
