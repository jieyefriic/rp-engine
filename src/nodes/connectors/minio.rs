//! MinIO (S3-compatible) data connector implementation
//!
//! Supports operations: get_object, put_object, list_objects, delete_object

use std::collections::HashMap;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, ConnectorContext};

/// Unified MinIO execute function
///
/// Takes data source config, operation config, and connector context.
/// Uses bindings from context for parameter resolution.
pub async fn execute(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    ctx: &ConnectorContext<'_>,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get connection parameters from data source config
    let endpoint = get_required_string(ds_config, "endpoint", ctx.state)?;
    let access_key = get_required_string(ds_config, "access_key", ctx.state)?;
    let secret_key = get_required_string(ds_config, "secret_key", ctx.state)?;
    let region = get_optional_string(ds_config, "region", ctx.state)
        .unwrap_or_else(|| "us-east-1".to_string());

    // Create S3 client for MinIO
    let client = create_minio_client(&endpoint, &access_key, &secret_key, &region).await?;

    // Get operation type
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("operation".to_string()))?;

    match op_type {
        "get_object" => execute_get_object(&client, operation, ctx.state).await,
        "put_object" => execute_put_object_v2(&client, &endpoint, operation, ctx.bindings, ctx.state).await,
        "list_objects" => execute_list_objects(&client, operation, ctx.state).await,
        "delete_object" => execute_delete_object(&client, operation, ctx.state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown MinIO operation: {}", op_type))),
    }
}

/// Create MinIO client with custom endpoint
async fn create_minio_client(
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
) -> Result<Client, EngineError> {
    let credentials = Credentials::new(
        access_key,
        secret_key,
        None, // session token
        None, // expiry
        "riceprompt-engine",
    );

    let config = aws_sdk_s3::Config::builder()
        .behavior_version_latest()
        .endpoint_url(endpoint)
        .region(Region::new(region.to_string()))
        .credentials_provider(credentials)
        .force_path_style(true) // Required for MinIO
        .build();

    Ok(Client::from_conf(config))
}

/// Execute get_object operation
async fn execute_get_object(
    client: &Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = get_required_string(connection, "bucket", state)?;
    let key = get_required_string(connection, "key", state)?;

    let response = client.get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await
        .map_err(|e| EngineError::MinIOError(e.to_string()))?;

    let content_type = response.content_type()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let content_length = response.content_length().unwrap_or(0);
    let last_modified = response.last_modified()
        .map(|t| t.to_string())
        .unwrap_or_default();

    // Read body
    let body = response.body.collect().await
        .map_err(|e| EngineError::MinIOError(e.to_string()))?;
    let bytes = body.into_bytes();

    // Try to parse as text, otherwise encode as base64
    let data = if content_type.starts_with("text/") ||
               content_type.contains("json") ||
               content_type.contains("xml") {
        match String::from_utf8(bytes.to_vec()) {
            Ok(text) => {
                // Try to parse JSON
                if content_type.contains("json") {
                    serde_json::from_str::<Value>(&text)
                        .unwrap_or(Value::String(text))
                } else {
                    Value::String(text)
                }
            }
            Err(_) => Value::String(BASE64.encode(&bytes)),
        }
    } else {
        Value::String(BASE64.encode(&bytes))
    };

    let mut outputs = HashMap::new();
    outputs.insert("data".to_string(), data);
    outputs.insert("content_type".to_string(), Value::String(content_type));
    outputs.insert("size".to_string(), Value::Number(content_length.into()));
    outputs.insert("last_modified".to_string(), Value::String(last_modified));

    Ok(outputs)
}

/// Execute put_object operation with bindings support
/// Supports:
/// - bucket: can be a template like "{userId}" resolved from bindings
/// - prefix: directory prefix
/// - data: base64 encoded data (resolved from bindings)
/// - content_type: MIME type
async fn execute_put_object_v2(
    client: &Client,
    endpoint: &str,
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get bucket - resolve from bindings
    let bucket = resolve_binding_value(operation, "bucket", bindings, state)?;
    let prefix = get_optional_string(operation, "prefix", state)
        .unwrap_or_else(|| "uploads".to_string());
    let content_type = get_optional_string(operation, "content_type", state)
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // Get data from bindings
    let data_base64 = if let Some(data_path) = bindings.get("data") {
        state.read(data_path)
            .map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            })
            .ok_or_else(|| EngineError::MissingField("data binding".to_string()))?
    } else {
        return Err(EngineError::MissingField("data binding".to_string()));
    };

    // Decode base64
    let data_bytes = BASE64.decode(&data_base64)
        .map_err(|e| EngineError::MinIOError(format!("Invalid base64 data: {}", e)))?;

    // Generate unique key
    let extension = extension_from_content_type(&content_type);
    let key = format!("{}/{}.{}", prefix, uuid::Uuid::new_v4(), extension);

    // Ensure bucket exists
    ensure_bucket_exists(client, &bucket).await?;

    // Upload
    let body = ByteStream::from(data_bytes);
    client.put_object()
        .bucket(&bucket)
        .key(&key)
        .body(body)
        .content_type(&content_type)
        .send()
        .await
        .map_err(|e| EngineError::MinIOError(e.to_string()))?;

    // Build URL
    let url = format!("{}/{}/{}", endpoint, bucket, key);

    let mut outputs = HashMap::new();
    outputs.insert("url".to_string(), Value::String(url));
    outputs.insert("bucket".to_string(), Value::String(bucket));
    outputs.insert("key".to_string(), Value::String(key));

    Ok(outputs)
}

/// Resolve a value that might be a template like "{userId}"
fn resolve_binding_value(
    operation: &HashMap<String, Value>,
    field: &str,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    let raw_value = operation.get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField(field.to_string()))?;

    // Check if it's a template like "{userId}"
    if raw_value.starts_with('{') && raw_value.ends_with('}') {
        let param_name = &raw_value[1..raw_value.len()-1];
        if let Some(state_path) = bindings.get(param_name) {
            return state.read(state_path)
                .map(|v| match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                })
                .ok_or_else(|| EngineError::MissingField(format!("binding {} not found in state", param_name)));
        }
    }

    Ok(raw_value.to_string())
}

fn extension_from_content_type(content_type: &str) -> &str {
    match content_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/m4a" => "m4a",
        _ => "bin",
    }
}

async fn ensure_bucket_exists(client: &Client, bucket: &str) -> Result<(), EngineError> {
    match client.head_bucket().bucket(bucket).send().await {
        Ok(_) => Ok(()),
        Err(_) => {
            client.create_bucket()
                .bucket(bucket)
                .send()
                .await
                .map_err(|e| EngineError::MinIOError(format!("Failed to create bucket: {}", e)))?;
            Ok(())
        }
    }
}

/// Execute list_objects operation
async fn execute_list_objects(
    client: &Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = get_required_string(connection, "bucket", state)?;
    let prefix = get_optional_string(connection, "prefix", state);
    let max_keys = get_optional_u64(connection, "max_keys")
        .map(|n| n as i32)
        .unwrap_or(1000);

    let mut request = client.list_objects_v2()
        .bucket(&bucket)
        .max_keys(max_keys);

    if let Some(p) = prefix {
        request = request.prefix(p);
    }

    let response = request.send().await
        .map_err(|e| EngineError::MinIOError(e.to_string()))?;

    let objects: Vec<Value> = response.contents()
        .iter()
        .map(|obj| {
            serde_json::json!({
                "key": obj.key().unwrap_or_default(),
                "size": obj.size().unwrap_or(0),
                "last_modified": obj.last_modified().map(|t| t.to_string()).unwrap_or_default(),
                "etag": obj.e_tag().unwrap_or_default(),
            })
        })
        .collect();

    let mut outputs = HashMap::new();
    outputs.insert("objects".to_string(), Value::Array(objects));
    outputs.insert("truncated".to_string(), Value::Bool(response.is_truncated().unwrap_or(false)));

    Ok(outputs)
}

/// Execute delete_object operation
async fn execute_delete_object(
    client: &Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = get_required_string(connection, "bucket", state)?;
    let key = get_required_string(connection, "key", state)?;

    client.delete_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await
        .map_err(|e| EngineError::MinIOError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("deleted".to_string(), Value::Bool(true));

    Ok(outputs)
}

