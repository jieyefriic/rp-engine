//! Redis data connector implementation
//!
//! Operations: get, set, del, publish, json_append, json_merge, json_delete
//!
//! Uses `key_template` / `value_template` with pre-resolved `{paramName}` variables.
//! Supports connection pooling via `EngineBuilder::redis()`.

use std::collections::HashMap;
use std::sync::OnceLock;
use redis::AsyncCommands;
use serde_json::Value;
use tracing::{debug, error};

use crate::error::EngineError;
use super::{get_optional_u64, ConnectorContext};

// ─── Lua scripts (loaded once, reused across calls) ─────────────────────────

const LUA_JSON_APPEND: &str = r#"
local key = KEYS[1]
local path = ARGV[1]
local value_json = ARGV[2]
local ttl = tonumber(ARGV[3])

local raw = redis.call('GET', key)
local data = raw and cjson.decode(raw) or {}

local target
if path == "" then
    if type(data) ~= "table" then data = {} end
    target = data
else
    if data[path] == nil then data[path] = {} end
    target = data[path]
end

local val = cjson.decode(value_json)
if type(val) == "table" and #val > 0 then
    for _, item in ipairs(val) do table.insert(target, item) end
else
    table.insert(target, val)
end

if path ~= "" then data[path] = target end
local encoded = cjson.encode(path == "" and target or data)
if ttl > 0 then redis.call('SETEX', key, ttl, encoded)
else redis.call('SET', key, encoded) end
return #target
"#;

const LUA_JSON_MERGE: &str = r#"
local key = KEYS[1]
local path = ARGV[1]
local merge_json = ARGV[2]
local ttl = tonumber(ARGV[3])

local raw = redis.call('GET', key)
local data = raw and cjson.decode(raw) or {}
local merge = cjson.decode(merge_json)

local target
if path == "" then
    target = data
else
    if data[path] == nil then data[path] = {} end
    target = data[path]
end

for k, v in pairs(merge) do target[k] = v end
if path ~= "" then data[path] = target end

local encoded = cjson.encode(path == "" and target or data)
if ttl > 0 then redis.call('SETEX', key, ttl, encoded)
else redis.call('SET', key, encoded) end
return 1
"#;

const LUA_JSON_DELETE: &str = r#"
local key = KEYS[1]
local path = ARGV[1]
local ttl = tonumber(ARGV[2])

local raw = redis.call('GET', key)
if not raw then return 0 end

local data = cjson.decode(raw)
data[path] = nil

local encoded = cjson.encode(data)
if ttl > 0 then redis.call('SETEX', key, ttl, encoded)
else redis.call('SET', key, encoded) end
return 1
"#;

fn json_append_script() -> &'static redis::Script {
    static SCRIPT: OnceLock<redis::Script> = OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(LUA_JSON_APPEND))
}

fn json_merge_script() -> &'static redis::Script {
    static SCRIPT: OnceLock<redis::Script> = OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(LUA_JSON_MERGE))
}

fn json_delete_script() -> &'static redis::Script {
    static SCRIPT: OnceLock<redis::Script> = OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(LUA_JSON_DELETE))
}

// ─── Template resolution ────────────────────────────────────────────────────

/// Resolve {paramName} patterns in a template using pre-resolved variables
fn resolve_param_template(
    template: &str,
    variables: &HashMap<String, Value>,
) -> String {
    let re = regex_lite::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();

    re.replace_all(template, |caps: &regex_lite::Captures| {
        let param_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some(value) = variables.get(param_name) {
            match value {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            }
        } else {
            format!("{{{}}}", param_name)
        }
    }).to_string()
}

/// Ensure a string is valid JSON for Lua cjson.decode().
///
/// - Already valid JSON → returned as-is
/// - Raw string (e.g. "hello") → wrapped as JSON string (`"hello"`)
fn ensure_json(s: &str) -> String {
    if serde_json::from_str::<Value>(s).is_ok() {
        s.to_string()
    } else {
        // Wrap as JSON string — handles escaping of quotes, unicode, etc.
        serde_json::to_string(s).unwrap_or_else(|_| format!("\"{}\"", s))
    }
}

/// Get the Redis key from operation config (key_template required)
fn get_key(
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<String, EngineError> {
    let template = operation.get("key_template")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("key_template".to_string()))?;
    Ok(resolve_param_template(template, variables))
}

/// Recursively resolve {paramName} in a structured JSON/YAML value.
///
/// Type-preserving: if a string leaf is exactly `"{varName}"` and the variable
/// holds a non-string type (array, object, number, bool), the raw value is
/// injected directly — no stringification.
///
/// Mixed templates like `"Hello {name}!"` always produce strings.
fn resolve_structured_value(value: &Value, variables: &HashMap<String, Value>) -> Value {
    match value {
        Value::String(s) => {
            // Check if the entire string is a single {var} reference
            let trimmed = s.trim();
            if trimmed.starts_with('{') && trimmed.ends_with('}') {
                let inner = &trimmed[1..trimmed.len() - 1];
                // Must be a valid identifier (no spaces, no nested braces)
                if !inner.is_empty()
                    && !inner.contains('{')
                    && !inner.contains('}')
                    && !inner.contains(' ')
                {
                    if let Some(val) = variables.get(inner) {
                        return val.clone();
                    }
                }
            }
            // Otherwise do string interpolation
            Value::String(resolve_param_template(s, variables))
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| resolve_structured_value(v, variables)).collect())
        }
        Value::Object(obj) => {
            Value::Object(obj.iter().map(|(k, v)| {
                (k.clone(), resolve_structured_value(v, variables))
            }).collect())
        }
        other => other.clone(),
    }
}

/// Get the Redis value as a JSON string from operation config.
///
/// Priority:
/// 1. `value` — structured YAML with recursive {var} resolution (type-preserving)
/// 2. `value_template` — flat string template with {var} interpolation
fn get_value(
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<String, EngineError> {
    // Structured value: resolve recursively, then serialize to JSON
    if let Some(structured) = operation.get("value") {
        let resolved = resolve_structured_value(structured, variables);
        return serde_json::to_string(&resolved)
            .map_err(|e| EngineError::RedisError(format!("Failed to serialize value: {}", e)));
    }

    // Flat string template
    if let Some(template) = operation.get("value_template").and_then(|v| v.as_str()) {
        return Ok(resolve_param_template(template, variables));
    }

    Err(EngineError::MissingField("value or value_template".to_string()))
}

/// Return typed empty value for output_schema fields
fn empty_value_for_type(field_type: &str) -> Value {
    match field_type {
        "array" => Value::Array(vec![]),
        "string" => Value::String(String::new()),
        "number" => Value::Number(0.into()),
        "boolean" => Value::Bool(false),
        "object" => Value::Object(serde_json::Map::new()),
        _ => Value::Null,
    }
}

/// Apply output_schema to extract fields from a parsed JSON value.
/// If `source` is Some, extract fields from it; if None, fill with empty defaults.
fn apply_output_schema(
    outputs: &mut HashMap<String, Value>,
    schema: &Value,
    source: Option<&Value>,
) {
    if let Value::Array(fields) = schema {
        let obj = source.and_then(|v| v.as_object());
        for field_def in fields {
            if let (Some(field_name), Some(field_type)) = (
                field_def.get("name").and_then(|n| n.as_str()),
                field_def.get("type").and_then(|t| t.as_str()),
            ) {
                let value = obj
                    .and_then(|o| o.get(field_name))
                    .cloned()
                    .unwrap_or_else(|| empty_value_for_type(field_type));
                outputs.insert(field_name.to_string(), value);
            }
        }
    }
}

// ─── Connection helpers ─────────────────────────────────────────────────────

/// Build Redis connection URL from data source config (no state dependency)
fn get_connection_url(
    connection: &HashMap<String, Value>,
) -> Result<String, EngineError> {
    if let Some(url) = connection.get("url").and_then(|v| v.as_str()) {
        return Ok(url.to_string());
    }

    let host = connection.get("host").and_then(|v| v.as_str()).unwrap_or("localhost");
    let port = get_optional_u64(connection, "port").unwrap_or(6379);
    let password = connection.get("password").and_then(|v| v.as_str());
    let db = get_optional_u64(connection, "db").unwrap_or(0);

    let url = if let Some(pass) = password {
        format!("redis://:{}@{}:{}/{}", pass, host, port, db)
    } else {
        format!("redis://{}:{}/{}", host, port, db)
    };

    Ok(url)
}

/// Get a Redis connection — prefer pool, fallback to ad-hoc
async fn get_connection(
    ds_config: &HashMap<String, Value>,
    ctx: &ConnectorContext<'_>,
) -> Result<redis::aio::ConnectionManager, EngineError> {
    // Try connection pool first
    if let Some(ds_id) = ctx.data_source_id {
        if let Some(manager) = ctx.config.pools.get_redis_pool(ds_id) {
            return Ok(manager.clone());
        }
    }

    // Fallback: create ad-hoc connection
    let url = get_connection_url(ds_config)?;
    let client = redis::Client::open(url.as_str())
        .map_err(|e| {
            error!(error = %e, url = %url, "Failed to create Redis client");
            EngineError::RedisError(format!("Failed to create Redis client: {}", e))
        })?;

    redis::aio::ConnectionManager::new(client).await
        .map_err(|e| {
            error!(error = %e, url = %url, "Failed to connect to Redis");
            EngineError::RedisError(format!("Failed to connect to Redis: {}", e))
        })
}

// ─── Main execute ───────────────────────────────────────────────────────────

/// Unified Redis execute function
pub async fn execute(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    ctx: &ConnectorContext<'_>,
) -> Result<HashMap<String, Value>, EngineError> {
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!(operation_config = ?operation, "Redis operation type not specified");
            EngineError::MissingField("operation".to_string())
        })?;

    debug!(operation = %op_type, "Starting Redis operation");

    let mut conn = get_connection(ds_config, ctx).await?;

    let result = match op_type {
        "get" => execute_get(&mut conn, operation, ctx.variables).await,
        "set" => execute_set(&mut conn, operation, ctx.variables).await,
        "del" => execute_del(&mut conn, operation, ctx.variables).await,
        "publish" => execute_publish(&mut conn, operation, ctx.variables).await,
        "json_append" => execute_json_append(&mut conn, operation, ctx.variables).await,
        "json_merge" => execute_json_merge(&mut conn, operation, ctx.variables).await,
        "json_delete" => execute_json_delete(&mut conn, operation, ctx.variables).await,
        // Hash operations
        "hset" => execute_hset(&mut conn, operation, ctx.variables).await,
        "hget" => execute_hget(&mut conn, operation, ctx.variables).await,
        "hgetall" => execute_hgetall(&mut conn, operation, ctx.variables).await,
        "hdel" => execute_hdel(&mut conn, operation, ctx.variables).await,
        // List operations
        "lpush" => execute_lpush(&mut conn, operation, ctx.variables).await,
        "rpush" => execute_rpush(&mut conn, operation, ctx.variables).await,
        "lpop" => execute_lpop(&mut conn, operation, ctx.variables).await,
        "rpop" => execute_rpop(&mut conn, operation, ctx.variables).await,
        "lrange" => execute_lrange(&mut conn, operation, ctx.variables).await,
        "llen" => execute_llen(&mut conn, operation, ctx.variables).await,
        _ => {
            error!(operation = %op_type, "Unknown Redis operation type");
            Err(EngineError::InvalidOperation(format!("Unknown Redis operation: {}", op_type)))
        }
    };

    result.map_err(|e| {
        let key = get_key(operation, ctx.variables).unwrap_or_else(|_| "(unknown)".to_string());
        error!(operation = %op_type, key = %key, error = %e, "Redis operation failed");
        e
    })
}

// ─── Operations ─────────────────────────────────────────────────────────────

/// GET — read a key, optionally extract fields via output_schema
async fn execute_get(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;

    let result: Option<String> = conn.get(&key).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();

    match result {
        Some(value) => {
            let parsed_value = serde_json::from_str::<Value>(&value)
                .unwrap_or(Value::String(value.clone()));

            if let Some(schema) = operation.get("output_schema") {
                apply_output_schema(&mut outputs, schema, Some(&parsed_value));
            }

            outputs.insert("value".to_string(), parsed_value);
            outputs.insert("exists".to_string(), Value::Bool(true));
        }
        None => {
            outputs.insert("value".to_string(), Value::Null);
            outputs.insert("exists".to_string(), Value::Bool(false));

            if let Some(schema) = operation.get("output_schema") {
                apply_output_schema(&mut outputs, schema, None);
            }
        }
    }

    Ok(outputs)
}

/// SET — write a key with optional TTL
async fn execute_set(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let value = get_value(operation, variables)?;
    let ttl = get_optional_u64(operation, "ttl");

    if let Some(seconds) = ttl {
        let _: () = conn.set_ex(&key, &value, seconds).await
            .map_err(|e| EngineError::RedisError(e.to_string()))?;
    } else {
        let _: () = conn.set(&key, &value).await
            .map_err(|e| EngineError::RedisError(e.to_string()))?;
    }

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(true));
    Ok(outputs)
}

/// DEL — delete a key
async fn execute_del(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;

    let deleted: i64 = conn.del(&key).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("deleted".to_string(), Value::Number(deleted.into()));
    outputs.insert("ok".to_string(), Value::Bool(deleted > 0));
    Ok(outputs)
}

/// PUBLISH — publish a message to a channel
async fn execute_publish(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let channel = get_key(operation, variables)?;
    let message = get_value(operation, variables)?;

    let subscribers: i64 = conn.publish(&channel, &message).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("subscribers".to_string(), Value::Number(subscribers.into()));
    outputs.insert("ok".to_string(), Value::Bool(true));
    Ok(outputs)
}

/// JSON_APPEND — atomically append element(s) to a JSON array field
///
/// Config: key_template, path (JSON field, "" = root), value_template (JSON), ttl
/// Output: { ok, length }
async fn execute_json_append(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let path = operation.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let value_json = ensure_json(&get_value(operation, variables)?);
    let ttl = get_optional_u64(operation, "ttl").unwrap_or(0);

    let length: i64 = json_append_script()
        .key(&key)
        .arg(path)
        .arg(&value_json)
        .arg(ttl)
        .invoke_async(conn).await
        .map_err(|e| EngineError::RedisError(format!("json_append failed: {}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(true));
    outputs.insert("length".to_string(), Value::Number(length.into()));
    Ok(outputs)
}

/// JSON_MERGE — atomically shallow-merge an object into a JSON field
///
/// Config: key_template, path (JSON field, "" = root), value_template (JSON object), ttl
/// Output: { ok }
async fn execute_json_merge(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let path = operation.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let merge_json = ensure_json(&get_value(operation, variables)?);
    let ttl = get_optional_u64(operation, "ttl").unwrap_or(0);

    let _: i64 = json_merge_script()
        .key(&key)
        .arg(path)
        .arg(&merge_json)
        .arg(ttl)
        .invoke_async(conn).await
        .map_err(|e| EngineError::RedisError(format!("json_merge failed: {}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(true));
    Ok(outputs)
}

/// JSON_DELETE — atomically delete a field from a JSON object
///
/// Config: key_template, path (JSON field to delete), ttl
/// Output: { ok, deleted }
async fn execute_json_delete(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let path = operation.get("path").and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("path is required for json_delete".to_string()))?;
    let ttl = get_optional_u64(operation, "ttl").unwrap_or(0);

    let deleted: i64 = json_delete_script()
        .key(&key)
        .arg(path)
        .arg(ttl)
        .invoke_async(conn).await
        .map_err(|e| EngineError::RedisError(format!("json_delete failed: {}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(true));
    outputs.insert("deleted".to_string(), Value::Number(deleted.into()));
    Ok(outputs)
}

// ─── Hash Operations ───────────────────────────────────────────────────────

/// HSET — set fields in a hash
async fn execute_hset(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;

    // Read field-value pairs from the `value` object
    let fields_value = operation.get("value")
        .ok_or_else(|| EngineError::MissingField("value (hash fields)".to_string()))?;

    let field_map = fields_value.as_object()
        .ok_or_else(|| EngineError::MissingField("value must be an object for hset".to_string()))?;

    let mut set_count: i64 = 0;
    for (field, val) in field_map {
        // Resolve {varName} references in string values
        let val_str = match val {
            Value::String(s) if s.starts_with('{') && s.ends_with('}') => {
                let var_name = &s[1..s.len()-1];
                match variables.get(var_name) {
                    Some(Value::String(resolved)) => resolved.clone(),
                    Some(other) => serde_json::to_string(other).unwrap_or_default(),
                    None => s.clone(),
                }
            }
            Value::String(s) => s.clone(),
            _ => serde_json::to_string(&val).unwrap_or_default(),
        };
        let added: i64 = redis::cmd("HSET")
            .arg(&key).arg(field).arg(&val_str)
            .query_async(conn).await
            .map_err(|e| EngineError::RedisError(e.to_string()))?;
        set_count += added;
    }

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(true));
    outputs.insert("fields_set".to_string(), Value::Number(set_count.into()));
    Ok(outputs)
}

/// HGET — get a single field from a hash
async fn execute_hget(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let field = operation.get("field")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("field".to_string()))?;

    let result: Option<String> = redis::cmd("HGET")
        .arg(&key).arg(field)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    match result {
        Some(val) => {
            let parsed = serde_json::from_str::<Value>(&val).unwrap_or(Value::String(val));
            outputs.insert("value".to_string(), parsed);
            outputs.insert("exists".to_string(), Value::Bool(true));
        }
        None => {
            outputs.insert("value".to_string(), Value::Null);
            outputs.insert("exists".to_string(), Value::Bool(false));
        }
    }
    Ok(outputs)
}

/// HGETALL — get all fields and values from a hash
async fn execute_hgetall(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;

    let result: Vec<(String, String)> = redis::cmd("HGETALL")
        .arg(&key)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut map = serde_json::Map::new();
    for (field, val) in &result {
        let parsed = serde_json::from_str::<Value>(val).unwrap_or(Value::String(val.clone()));
        map.insert(field.clone(), parsed);
    }

    let mut outputs = HashMap::new();
    outputs.insert("value".to_string(), Value::Object(map));
    outputs.insert("exists".to_string(), Value::Bool(!result.is_empty()));
    outputs.insert("field_count".to_string(), Value::Number((result.len() as i64).into()));
    Ok(outputs)
}

/// HDEL — delete fields from a hash
async fn execute_hdel(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let field = operation.get("field")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("field".to_string()))?;

    let deleted: i64 = redis::cmd("HDEL")
        .arg(&key).arg(field)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(deleted > 0));
    outputs.insert("deleted".to_string(), Value::Number(deleted.into()));
    Ok(outputs)
}

// ─── List Operations ───────────────────────────────────────────────────────

/// LPUSH — push value(s) to the head of a list
async fn execute_lpush(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let value = get_value(operation, variables)?;

    let length: i64 = redis::cmd("LPUSH")
        .arg(&key).arg(&value)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(true));
    outputs.insert("length".to_string(), Value::Number(length.into()));
    Ok(outputs)
}

/// RPUSH — push value(s) to the tail of a list
async fn execute_rpush(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let value = get_value(operation, variables)?;

    let length: i64 = redis::cmd("RPUSH")
        .arg(&key).arg(&value)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("ok".to_string(), Value::Bool(true));
    outputs.insert("length".to_string(), Value::Number(length.into()));
    Ok(outputs)
}

/// LPOP — remove and return value from the head of a list
async fn execute_lpop(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;

    let result: Option<String> = redis::cmd("LPOP")
        .arg(&key)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    match result {
        Some(val) => {
            let parsed = serde_json::from_str::<Value>(&val).unwrap_or(Value::String(val));
            outputs.insert("value".to_string(), parsed);
            outputs.insert("exists".to_string(), Value::Bool(true));
        }
        None => {
            outputs.insert("value".to_string(), Value::Null);
            outputs.insert("exists".to_string(), Value::Bool(false));
        }
    }
    Ok(outputs)
}

/// RPOP — remove and return value from the tail of a list
async fn execute_rpop(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;

    let result: Option<String> = redis::cmd("RPOP")
        .arg(&key)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    match result {
        Some(val) => {
            let parsed = serde_json::from_str::<Value>(&val).unwrap_or(Value::String(val));
            outputs.insert("value".to_string(), parsed);
            outputs.insert("exists".to_string(), Value::Bool(true));
        }
        None => {
            outputs.insert("value".to_string(), Value::Null);
            outputs.insert("exists".to_string(), Value::Bool(false));
        }
    }
    Ok(outputs)
}

/// LRANGE — get elements from a list by index range
async fn execute_lrange(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;
    let start = operation.get("start").and_then(|v| v.as_i64()).unwrap_or(0);
    let stop = operation.get("stop").and_then(|v| v.as_i64()).unwrap_or(-1);

    let result: Vec<String> = redis::cmd("LRANGE")
        .arg(&key).arg(start).arg(stop)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let values: Vec<Value> = result.iter()
        .map(|v| serde_json::from_str::<Value>(v).unwrap_or(Value::String(v.clone())))
        .collect();

    let mut outputs = HashMap::new();
    outputs.insert("value".to_string(), Value::Array(values));
    outputs.insert("length".to_string(), Value::Number((result.len() as i64).into()));
    Ok(outputs)
}

/// LLEN — get the length of a list
async fn execute_llen(
    conn: &mut redis::aio::ConnectionManager,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let key = get_key(operation, variables)?;

    let length: i64 = redis::cmd("LLEN")
        .arg(&key)
        .query_async(conn).await
        .map_err(|e| EngineError::RedisError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("length".to_string(), Value::Number(length.into()));
    Ok(outputs)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_connection_url_from_url() {
        let mut connection = HashMap::new();
        connection.insert("url".to_string(), Value::String("redis://localhost:6379".to_string()));

        let url = get_connection_url(&connection).unwrap();
        assert_eq!(url, "redis://localhost:6379");
    }

    #[test]
    fn test_get_connection_url_from_components() {
        let mut connection = HashMap::new();
        connection.insert("host".to_string(), Value::String("myhost".to_string()));
        connection.insert("port".to_string(), Value::Number(6380.into()));
        connection.insert("password".to_string(), Value::String("secret".to_string()));
        connection.insert("db".to_string(), Value::Number(1.into()));

        let url = get_connection_url(&connection).unwrap();
        assert_eq!(url, "redis://:secret@myhost:6380/1");
    }

    #[test]
    fn test_get_connection_url_defaults() {
        let connection = HashMap::new();
        let url = get_connection_url(&connection).unwrap();
        assert_eq!(url, "redis://localhost:6379/0");
    }

    #[test]
    fn test_resolve_param_template() {
        let mut variables = HashMap::new();
        variables.insert("userId".to_string(), Value::String("123".to_string()));
        variables.insert("region".to_string(), Value::String("us-west".to_string()));

        assert_eq!(resolve_param_template("user:{userId}", &variables), "user:123");
        assert_eq!(resolve_param_template("cache:{region}:{userId}", &variables), "cache:us-west:123");
        assert_eq!(resolve_param_template("static_key", &variables), "static_key");
        assert_eq!(resolve_param_template("user:{unknownParam}", &variables), "user:{unknownParam}");
    }

    #[test]
    fn test_get_key_requires_key_template() {
        let variables = HashMap::new();
        let mut operation = HashMap::new();

        // Missing key_template should error
        assert!(get_key(&operation, &variables).is_err());

        // With key_template should succeed
        operation.insert("key_template".to_string(), Value::String("test:{id}".to_string()));
        assert!(get_key(&operation, &variables).is_ok());
    }

    #[test]
    fn test_get_key_with_template() {
        let mut variables = HashMap::new();
        variables.insert("userId".to_string(), Value::String("456".to_string()));

        let mut operation = HashMap::new();
        operation.insert("key_template".to_string(), Value::String("user:{userId}".to_string()));

        let result = get_key(&operation, &variables).unwrap();
        assert_eq!(result, "user:456");
    }

    #[test]
    fn test_get_value_requires_value_template() {
        let variables = HashMap::new();
        let operation = HashMap::new();
        assert!(get_value(&operation, &variables).is_err());
    }

    #[test]
    fn test_empty_value_for_type() {
        assert_eq!(empty_value_for_type("array"), Value::Array(vec![]));
        assert_eq!(empty_value_for_type("string"), Value::String(String::new()));
        assert_eq!(empty_value_for_type("number"), Value::Number(0.into()));
        assert_eq!(empty_value_for_type("boolean"), Value::Bool(false));
        assert_eq!(empty_value_for_type("object"), Value::Object(serde_json::Map::new()));
        assert_eq!(empty_value_for_type("unknown"), Value::Null);
    }

    #[test]
    fn test_apply_output_schema_with_data() {
        let mut outputs = HashMap::new();
        let schema = serde_json::json!([
            { "name": "name", "type": "string" },
            { "name": "age", "type": "number" },
            { "name": "tags", "type": "array" },
            { "name": "missing", "type": "string" },
        ]);
        let source = serde_json::json!({
            "name": "Alice",
            "age": 30,
            "tags": ["a", "b"],
        });

        apply_output_schema(&mut outputs, &schema, Some(&source));

        assert_eq!(outputs.get("name"), Some(&Value::String("Alice".to_string())));
        assert_eq!(outputs.get("age"), Some(&Value::Number(30.into())));
        assert_eq!(outputs.get("tags"), Some(&serde_json::json!(["a", "b"])));
        // missing field gets empty default
        assert_eq!(outputs.get("missing"), Some(&Value::String(String::new())));
    }

    #[test]
    fn test_apply_output_schema_no_data() {
        let mut outputs = HashMap::new();
        let schema = serde_json::json!([
            { "name": "items", "type": "array" },
            { "name": "count", "type": "number" },
        ]);

        apply_output_schema(&mut outputs, &schema, None);

        assert_eq!(outputs.get("items"), Some(&Value::Array(vec![])));
        assert_eq!(outputs.get("count"), Some(&Value::Number(0.into())));
    }

    #[test]
    fn test_resolve_structured_value_string_interpolation() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), Value::String("Alice".to_string()));

        // Mixed template stays string
        let result = resolve_structured_value(&serde_json::json!("Hello {name}!"), &vars);
        assert_eq!(result, Value::String("Hello Alice!".to_string()));

        // Exact {var} with string value stays string
        let result = resolve_structured_value(&serde_json::json!("{name}"), &vars);
        assert_eq!(result, Value::String("Alice".to_string()));
    }

    #[test]
    fn test_resolve_structured_value_type_preservation() {
        let mut vars = HashMap::new();
        vars.insert("count".to_string(), Value::Number(42.into()));
        vars.insert("active".to_string(), Value::Bool(true));
        vars.insert("tags".to_string(), serde_json::json!(["a", "b"]));
        vars.insert("meta".to_string(), serde_json::json!({"k": "v"}));

        // Exact {var} with number → preserves number
        assert_eq!(
            resolve_structured_value(&serde_json::json!("{count}"), &vars),
            Value::Number(42.into())
        );

        // Exact {var} with bool → preserves bool
        assert_eq!(
            resolve_structured_value(&serde_json::json!("{active}"), &vars),
            Value::Bool(true)
        );

        // Exact {var} with array → preserves array
        assert_eq!(
            resolve_structured_value(&serde_json::json!("{tags}"), &vars),
            serde_json::json!(["a", "b"])
        );

        // Exact {var} with object → preserves object
        assert_eq!(
            resolve_structured_value(&serde_json::json!("{meta}"), &vars),
            serde_json::json!({"k": "v"})
        );
    }

    #[test]
    fn test_resolve_structured_value_nested() {
        let mut vars = HashMap::new();
        vars.insert("userMessage".to_string(), Value::String("hello".to_string()));
        vars.insert("assistantMessage".to_string(), Value::String("world".to_string()));
        vars.insert("messageType".to_string(), Value::String("text".to_string()));

        let template = serde_json::json!([
            { "role": "user", "content": "{userMessage}", "type": "{messageType}" },
            { "role": "assistant", "content": "{assistantMessage}" },
        ]);

        let result = resolve_structured_value(&template, &vars);
        assert_eq!(result, serde_json::json!([
            { "role": "user", "content": "hello", "type": "text" },
            { "role": "assistant", "content": "world" },
        ]));
    }

    #[test]
    fn test_get_value_structured_over_template() {
        let mut vars = HashMap::new();
        vars.insert("msg".to_string(), Value::String("hi".to_string()));

        // value takes priority over value_template
        let mut op = HashMap::new();
        op.insert("value".to_string(), serde_json::json!({"text": "{msg}"}));
        op.insert("value_template".to_string(), Value::String("ignored".to_string()));

        let result = get_value(&op, &vars).unwrap();
        assert_eq!(result, r#"{"text":"hi"}"#);
    }

    #[test]
    fn test_get_value_fallback_to_template() {
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), Value::String("42".to_string()));

        let mut op = HashMap::new();
        op.insert("value_template".to_string(), Value::String("val:{x}".to_string()));

        let result = get_value(&op, &vars).unwrap();
        assert_eq!(result, "val:42");
    }

    #[test]
    fn test_get_value_neither_errors() {
        let vars = HashMap::new();
        let op = HashMap::new();
        assert!(get_value(&op, &vars).is_err());
    }

    #[test]
    fn test_ensure_json() {
        // Already valid JSON → pass through
        assert_eq!(ensure_json(r#"["a","b"]"#), r#"["a","b"]"#);
        assert_eq!(ensure_json(r#"{"k":"v"}"#), r#"{"k":"v"}"#);
        assert_eq!(ensure_json("42"), "42");
        assert_eq!(ensure_json("true"), "true");
        assert_eq!(ensure_json(r#""quoted""#), r#""quoted""#);

        // Raw string → wrap as JSON string
        assert_eq!(ensure_json("hello"), r#""hello""#);
        assert_eq!(ensure_json("科幻电影推荐"), r#""科幻电影推荐""#);
        assert_eq!(ensure_json("has \"quotes\""), r#""has \"quotes\"""#);
    }
}
