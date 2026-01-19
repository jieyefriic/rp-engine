//! MinIO (S3-compatible) data connector implementation
//!
//! Supports operations: get_object, put_object, list_objects, delete_object

use std::collections::HashMap;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use crate::schema::NodeConfig;
use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64};

/// Execute a MinIO operation
pub async fn execute(
    config: &NodeConfig,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let connection = &config.connection;

    // Get connection parameters
    let endpoint = get_required_string(connection, "endpoint", state)?;
    let access_key = get_required_string(connection, "access_key", state)?;
    let secret_key = get_required_string(connection, "secret_key", state)?;
    let region = get_optional_string(connection, "region", state)
        .unwrap_or_else(|| "us-east-1".to_string());

    // Get operation
    let operation = get_required_string(connection, "operation", state)?;

    // Create S3 client for MinIO
    let client = create_minio_client(&endpoint, &access_key, &secret_key, &region).await?;

    match operation.as_str() {
        "get_object" => execute_get_object(&client, connection, state).await,
        "put_object" => execute_put_object(&client, connection, state).await,
        "list_objects" => execute_list_objects(&client, connection, state).await,
        "delete_object" => execute_delete_object(&client, connection, state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown MinIO operation: {}", operation))),
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

/// Execute put_object operation
async fn execute_put_object(
    client: &Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = get_required_string(connection, "bucket", state)?;
    let key = get_required_string(connection, "key", state)?;
    let content = get_required_string(connection, "content", state)?;
    let content_type = get_optional_string(connection, "content_type", state)
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let body = ByteStream::from(content.into_bytes());

    let response = client.put_object()
        .bucket(&bucket)
        .key(&key)
        .body(body)
        .content_type(&content_type)
        .send()
        .await
        .map_err(|e| EngineError::MinIOError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("etag".to_string(),
        Value::String(response.e_tag().unwrap_or_default().to_string()));
    outputs.insert("version_id".to_string(),
        response.version_id()
            .map(|v| Value::String(v.to_string()))
            .unwrap_or(Value::Null));

    Ok(outputs)
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

/// Execute MinIO with V2 config (data source + operation)
pub async fn execute_v2(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get connection parameters from data source config
    let endpoint = get_required_string(ds_config, "endpoint", state)?;
    let access_key = get_required_string(ds_config, "access_key", state)?;
    let secret_key = get_required_string(ds_config, "secret_key", state)?;
    let region = get_optional_string(ds_config, "region", state)
        .unwrap_or_else(|| "us-east-1".to_string());

    // Create S3 client for MinIO
    let client = create_minio_client(&endpoint, &access_key, &secret_key, &region).await?;

    // Get operation type
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("operation".to_string()))?;

    match op_type {
        "get_object" => execute_get_object(&client, operation, state).await,
        "put_object" => execute_put_object(&client, operation, state).await,
        "list_objects" => execute_list_objects(&client, operation, state).await,
        "delete_object" => execute_delete_object(&client, operation, state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown MinIO operation: {}", op_type))),
    }
}
