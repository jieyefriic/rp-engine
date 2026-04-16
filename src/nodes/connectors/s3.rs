//! S3-compatible object storage data connector implementation
//!
//! Supports operations: get_object, put_object, list_objects, delete_object,
//! presigned_url, head_object, copy_object

use std::collections::HashMap;
use std::time::Duration;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, ConnectorContext};

/// Unified S3 execute function
///
/// Takes data source config, operation config, and connector context.
/// Uses pre-resolved variables from context for parameter resolution.
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

    // Create S3 client
    let client = create_s3_client(&endpoint, &access_key, &secret_key, &region).await?;

    // Get operation type
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("operation".to_string()))?;

    match op_type {
        "get_object" => execute_get_object(&client, operation, ctx.variables, ctx.state).await,
        "put_object" => execute_put_object_v2(&client, &endpoint, operation, ctx.variables, ctx.state).await,
        "list_objects" => execute_list_objects(&client, operation, ctx.variables, ctx.state).await,
        "delete_object" => execute_delete_object(&client, operation, ctx.variables, ctx.state).await,
        "presigned_url" => execute_presigned_url(ds_config, &access_key, &secret_key, &region, operation, ctx.variables, ctx.state).await,
        "head_object" => execute_head_object(&client, operation, ctx.variables, ctx.state).await,
        "copy_object" => execute_copy_object(&client, operation, ctx.variables, ctx.state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown S3 operation: {}", op_type))),
    }
}

/// Create S3 client with custom endpoint
async fn create_s3_client(
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
        .force_path_style(true) // Required for S3-compatible services (MinIO, etc.)
        .build();

    Ok(Client::from_conf(config))
}

/// Execute get_object operation
async fn execute_get_object(
    client: &Client,
    connection: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = resolve_string_field(connection, "bucket", variables, state)?;
    let key = resolve_string_field(connection, "key", variables, state)?;

    let response = client.get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await
        .map_err(|e| EngineError::S3Error(format!("{:#}", e)))?;

    let content_type = response.content_type()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let content_length = response.content_length().unwrap_or(0);
    let last_modified = response.last_modified()
        .map(|t| t.to_string())
        .unwrap_or_default();

    // Read body
    let body = response.body.collect().await
        .map_err(|e| EngineError::S3Error(format!("{:#}", e)))?;
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

/// Execute put_object operation with pre-resolved variables
/// Supports:
/// - bucket: can be a template like "{userId}" resolved from variables
/// - prefix: directory prefix
/// - data: base64 encoded data (from pre-resolved variables)
/// - content_type: MIME type, or "auto" to detect from binary data magic bytes
///   Supported auto-detection: JPEG, PNG, GIF, WebP, BMP, MP3, WAV, OGG, M4A
async fn execute_put_object_v2(
    client: &Client,
    endpoint: &str,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get bucket - resolve from variables
    let bucket = resolve_binding_value(operation, "bucket", variables)?;
    let prefix = get_optional_string(operation, "prefix", state)
        .unwrap_or_else(|| "uploads".to_string());
    let configured_content_type = get_optional_string(operation, "content_type", state);

    // Get data from pre-resolved variables
    let data_base64 = variables.get("data")
        .and_then(|v| match v {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
        .ok_or_else(|| EngineError::MissingField("data variable".to_string()))?;

    // Decode base64
    let data_bytes = BASE64.decode(&data_base64)
        .map_err(|e| EngineError::S3Error(format!("Invalid base64 data: {}", e)))?;

    // Determine content type and extension
    // If content_type is "auto" or not specified, detect from binary data
    let (content_type, extension): (String, &str) = match configured_content_type.as_deref() {
        Some("auto") | None => {
            // Try to detect from magic bytes
            if let Some((detected_type, detected_ext)) = detect_content_type(&data_bytes) {
                (detected_type.to_string(), detected_ext)
            } else {
                // Fallback to default
                ("application/octet-stream".to_string(), "bin")
            }
        }
        Some(ct) => {
            let ext = extension_from_content_type(ct);
            (ct.to_string(), ext)
        }
    };

    // Generate unique key
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
        .map_err(|e| EngineError::S3Error(format!("{:#}", e)))?;

    // Build URL
    let url = format!("{}/{}/{}", endpoint, bucket, key);

    let mut outputs = HashMap::new();
    outputs.insert("url".to_string(), Value::String(url));
    outputs.insert("bucket".to_string(), Value::String(bucket));
    outputs.insert("key".to_string(), Value::String(key));

    Ok(outputs)
}

/// Resolve a value that might be a template like "{userId}" using pre-resolved variables
fn resolve_binding_value(
    operation: &HashMap<String, Value>,
    field: &str,
    variables: &HashMap<String, Value>,
) -> Result<String, EngineError> {
    let raw_value = operation.get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField(field.to_string()))?;

    // Check if it's a template like "{userId}"
    if raw_value.starts_with('{') && raw_value.ends_with('}') {
        let param_name = &raw_value[1..raw_value.len()-1];
        if let Some(value) = variables.get(param_name) {
            return Ok(match value {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            });
        }
    }

    Ok(raw_value.to_string())
}

/// Resolve a string field from operation config, supporting {paramName} variable resolution.
/// Falls back to get_required_string (which handles legacy {{node.field}} templates).
fn resolve_string_field(
    operation: &HashMap<String, Value>,
    field: &str,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    let raw = operation.get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField(field.to_string()))?;

    // Check if the entire value is a {paramName} reference
    if raw.starts_with('{') && raw.ends_with('}') && !raw.contains(' ') {
        // Could be {paramName} or {{legacyVar}}
        let inner = &raw[1..raw.len()-1];
        if !inner.starts_with('{') {
            // Single brace: {paramName} — resolve from variables
            if let Some(value) = variables.get(inner) {
                return Ok(match value {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                });
            }
        }
    }

    // Fall back to legacy {{node.field}} template resolution
    Ok(super::resolve_template_vars(raw, state))
}

/// Resolve an optional string field, supporting {paramName} variable resolution.
fn resolve_optional_string_field(
    operation: &HashMap<String, Value>,
    field: &str,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Option<String> {
    let raw = operation.get(field).and_then(|v| v.as_str())?;

    if raw.starts_with('{') && raw.ends_with('}') && !raw.contains(' ') {
        let inner = &raw[1..raw.len()-1];
        if !inner.starts_with('{') {
            if let Some(value) = variables.get(inner) {
                return Some(match value {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                });
            }
        }
    }

    Some(super::resolve_template_vars(raw, state))
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

/// Detect content type from binary data using magic bytes
/// Returns (mime_type, extension)
fn detect_content_type(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.len() < 12 {
        return None;
    }

    // Image formats
    match &data[0..4] {
        // JPEG: FF D8 FF
        [0xFF, 0xD8, 0xFF, ..] => return Some(("image/jpeg", "jpg")),
        // PNG: 89 50 4E 47 (‰PNG)
        [0x89, 0x50, 0x4E, 0x47] => return Some(("image/png", "png")),
        // GIF: 47 49 46 38 (GIF8)
        [0x47, 0x49, 0x46, 0x38] => return Some(("image/gif", "gif")),
        // WebP: RIFF....WEBP
        [0x52, 0x49, 0x46, 0x46] if &data[8..12] == b"WEBP" => {
            return Some(("image/webp", "webp"))
        }
        // BMP: 42 4D (BM)
        [0x42, 0x4D, ..] => return Some(("image/bmp", "bmp")),
        _ => {}
    }

    // Audio formats
    // MP3: FF FB, FF FA, FF F3, or ID3 tag
    if data.len() >= 3 {
        if (data[0] == 0xFF && (data[1] & 0xE0) == 0xE0) || &data[0..3] == b"ID3" {
            return Some(("audio/mpeg", "mp3"));
        }
    }

    // WAV: RIFF....WAVE
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        return Some(("audio/wav", "wav"));
    }

    // OGG: OggS
    if data.len() >= 4 && &data[0..4] == b"OggS" {
        return Some(("audio/ogg", "ogg"));
    }

    // M4A/AAC: ftyp
    if data.len() >= 8 && &data[4..8] == b"ftyp" {
        return Some(("audio/m4a", "m4a"));
    }

    None
}

async fn ensure_bucket_exists(client: &Client, bucket: &str) -> Result<(), EngineError> {
    match client.head_bucket().bucket(bucket).send().await {
        Ok(_) => Ok(()),
        Err(_) => {
            client.create_bucket()
                .bucket(bucket)
                .send()
                .await
                .map_err(|e| EngineError::S3Error(format!("Failed to create bucket: {}", e)))?;
            Ok(())
        }
    }
}

/// Execute list_objects operation
async fn execute_list_objects(
    client: &Client,
    connection: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = resolve_string_field(connection, "bucket", variables, state)?;
    let prefix = resolve_optional_string_field(connection, "prefix", variables, state);
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
        .map_err(|e| EngineError::S3Error(format!("{:#}", e)))?;

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
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = resolve_string_field(connection, "bucket", variables, state)?;
    let key = resolve_string_field(connection, "key", variables, state)?;

    client.delete_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await
        .map_err(|e| EngineError::S3Error(format!("{:#}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("deleted".to_string(), Value::Bool(true));

    Ok(outputs)
}

/// Execute presigned_url operation — generate a temporary access URL
///
/// Uses `public_endpoint` from data source config if available,
/// so the presigned URL points to the public-facing domain.
async fn execute_presigned_url(
    ds_config: &HashMap<String, Value>,
    access_key: &str,
    secret_key: &str,
    region: &str,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = resolve_string_field(operation, "bucket", variables, state)?;
    let key = resolve_string_field(operation, "key", variables, state)?;
    let expires_in = get_optional_u64(operation, "expires_in").unwrap_or(3600);

    // Use public_endpoint for presigning if available, otherwise fall back to endpoint
    let presign_endpoint = get_optional_string(ds_config, "public_endpoint", state)
        .or_else(|| get_optional_string(ds_config, "endpoint", state))
        .ok_or_else(|| EngineError::MissingField("endpoint".to_string()))?;

    let presign_client = create_s3_client(&presign_endpoint, access_key, secret_key, region).await?;

    let presigning_config = PresigningConfig::builder()
        .expires_in(Duration::from_secs(expires_in))
        .build()
        .map_err(|e| EngineError::S3Error(format!("Invalid presigning config: {}", e)))?;

    let presigned = presign_client
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .presigned(presigning_config)
        .await
        .map_err(|e| EngineError::S3Error(format!("{:#}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("url".to_string(), Value::String(presigned.uri().to_string()));
    outputs.insert("expires_in".to_string(), Value::Number(expires_in.into()));

    Ok(outputs)
}

/// Execute head_object operation — check existence and get metadata without downloading
async fn execute_head_object(
    client: &Client,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let bucket = resolve_string_field(operation, "bucket", variables, state)?;
    let key = resolve_string_field(operation, "key", variables, state)?;

    let mut outputs = HashMap::new();

    match client.head_object().bucket(&bucket).key(&key).send().await {
        Ok(response) => {
            outputs.insert("exists".to_string(), Value::Bool(true));
            outputs.insert("content_type".to_string(), Value::String(
                response.content_type().unwrap_or("application/octet-stream").to_string()
            ));
            outputs.insert("size".to_string(), Value::Number(
                response.content_length().unwrap_or(0).into()
            ));
            outputs.insert("last_modified".to_string(), Value::String(
                response.last_modified().map(|t| t.to_string()).unwrap_or_default()
            ));
        }
        Err(_) => {
            outputs.insert("exists".to_string(), Value::Bool(false));
            outputs.insert("content_type".to_string(), Value::String(String::new()));
            outputs.insert("size".to_string(), Value::Number(0.into()));
            outputs.insert("last_modified".to_string(), Value::String(String::new()));
        }
    }

    Ok(outputs)
}

/// Execute copy_object operation — server-side copy between buckets/keys
async fn execute_copy_object(
    client: &Client,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let source_bucket = resolve_string_field(operation, "source_bucket", variables, state)?;
    let source_key = resolve_string_field(operation, "source_key", variables, state)?;
    let bucket = resolve_string_field(operation, "bucket", variables, state)?;
    let key = resolve_string_field(operation, "key", variables, state)?;

    let copy_source = format!("{}/{}", source_bucket, source_key);

    client.copy_object()
        .bucket(&bucket)
        .key(&key)
        .copy_source(&copy_source)
        .send()
        .await
        .map_err(|e| EngineError::S3Error(format!("{:#}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("bucket".to_string(), Value::String(bucket));
    outputs.insert("key".to_string(), Value::String(key));

    Ok(outputs)
}

