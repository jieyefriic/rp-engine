//! Qdrant vector database connector implementation
//!
//! Supports operations: search, scroll, get_points, upsert, batch_upsert, delete, create_collection
//!
//! The search operation supports auto-embedding: provide "text" instead of "vector"
//! and the connector will automatically generate embeddings using the configured provider.

use std::collections::HashMap;
use std::sync::Arc;
use qdrant_client::Qdrant;
use qdrant_client::qdrant::{
    SearchPointsBuilder, ScrollPointsBuilder, GetPointsBuilder,
    Filter, Condition, PointId,
    condition::ConditionOneOf, FieldCondition, r#match::MatchValue, Match,
    UpsertPointsBuilder, PointStruct, DeletePointsBuilder,
    CreateCollectionBuilder, VectorParamsBuilder, Distance,
    PointsIdsList,
};
use serde_json::Value;
use tracing::{debug, error};

use crate::state::ExecutionState;
use crate::error::EngineError;
use crate::providers::{EmbeddingProvider, EmbeddingOptions};
use super::{get_required_string, get_optional_string, get_optional_u64, get_optional_bool, ConnectorContext};

/// Unified Qdrant execute function
///
/// Takes data source config, operation config, and connector context.
/// Gets embedding provider from context.config.embedding_providers for search and upsert operations.
pub async fn execute(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    ctx: &ConnectorContext<'_>,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get connection parameters from data source config
    let url = get_required_string(ds_config, "url", ctx.state)?;
    let api_key = get_optional_string(ds_config, "api_key", ctx.state);

    // Get operation type
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!(operation_config = ?operation, "Qdrant operation type not specified");
            EngineError::MissingField("operation".to_string())
        })?;

    let collection = get_optional_string(operation, "collection", ctx.state)
        .unwrap_or_else(|| "(not specified)".to_string());

    debug!(
        operation = %op_type,
        collection = %collection,
        url = %url,
        "Starting Qdrant operation"
    );

    // Create Qdrant client
    let client = create_qdrant_client(&url, api_key.as_deref()).await
        .map_err(|e| {
            error!(
                error = %e,
                url = %url,
                "Failed to create Qdrant client"
            );
            e
        })?;

    // Get embedding provider for upsert operations if configured
    let embedding_provider: Option<Arc<dyn EmbeddingProvider>> = {
        let provider_name = operation.get("embedding_provider")
            .and_then(|v| v.as_str())
            .unwrap_or("dashscope");
        ctx.config.embedding_providers.get(provider_name)
    };

    let result = match op_type {
        "search" => execute_search(&client, operation, ctx.state, embedding_provider.as_deref()).await,
        "scroll" => execute_scroll(&client, operation, ctx.state).await,
        "get_points" => execute_get_points(&client, operation, ctx.state).await,
        "upsert" => execute_upsert(&client, operation, ctx.state, embedding_provider.as_deref()).await,
        "batch_upsert" => execute_batch_upsert(&client, operation, ctx.state, embedding_provider.as_deref()).await,
        "delete" => execute_delete(&client, operation, ctx.state).await,
        "create_collection" => execute_create_collection(&client, operation, ctx.state).await,
        _ => {
            error!(operation = %op_type, "Unknown Qdrant operation type");
            Err(EngineError::InvalidOperation(format!("Unknown Qdrant operation: {}", op_type)))
        }
    };

    result.map_err(|e| {
        error!(
            operation = %op_type,
            collection = %collection,
            error = %e,
            "Qdrant operation failed"
        );
        e
    })
}

/// Create Qdrant client
async fn create_qdrant_client(
    url: &str,
    api_key: Option<&str>,
) -> Result<Qdrant, EngineError> {
    let mut builder = Qdrant::from_url(url);

    if let Some(key) = api_key {
        builder = builder.api_key(key);
    }

    builder.build()
        .map_err(|e| EngineError::QdrantError(e.to_string()))
}

/// Parse vector from connection config
fn parse_vector(connection: &HashMap<String, Value>, state: &ExecutionState) -> Result<Vec<f32>, EngineError> {
    let vector_value = connection.get("vector")
        .ok_or_else(|| EngineError::MissingField("vector".to_string()))?;

    match vector_value {
        Value::Array(arr) => {
            arr.iter()
                .map(|v| v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| EngineError::QdrantError("Invalid vector value".to_string())))
                .collect()
        }
        Value::String(path) => {
            // Resolve from state
            let resolved = super::resolve_template_vars(path, state);
            if resolved.starts_with('[') {
                // It's a JSON array string
                let parsed: Vec<f32> = serde_json::from_str(&resolved)
                    .map_err(|e| EngineError::QdrantError(format!("Failed to parse vector: {}", e)))?;
                Ok(parsed)
            } else {
                // It's a path reference
                state.read(&resolved)
                    .and_then(|v| {
                        v.as_array().map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_f64().map(|f| f as f32))
                                .collect()
                        })
                    })
                    .ok_or_else(|| EngineError::QdrantError("Vector not found in state".to_string()))
            }
        }
        _ => Err(EngineError::QdrantError("Invalid vector format".to_string())),
    }
}

/// Parse filter from connection config
fn parse_filter(connection: &HashMap<String, Value>) -> Option<Filter> {
    let filter_value = connection.get("filter")?;
    let filter_obj = filter_value.as_object()?;

    let mut must_conditions = Vec::new();
    let mut should_conditions = Vec::new();
    let mut must_not_conditions = Vec::new();

    // Parse "must" conditions
    if let Some(must) = filter_obj.get("must").and_then(|v| v.as_array()) {
        for condition in must {
            if let Some(c) = parse_condition(condition) {
                must_conditions.push(c);
            }
        }
    }

    // Parse "should" conditions
    if let Some(should) = filter_obj.get("should").and_then(|v| v.as_array()) {
        for condition in should {
            if let Some(c) = parse_condition(condition) {
                should_conditions.push(c);
            }
        }
    }

    // Parse "must_not" conditions
    if let Some(must_not) = filter_obj.get("must_not").and_then(|v| v.as_array()) {
        for condition in must_not {
            if let Some(c) = parse_condition(condition) {
                must_not_conditions.push(c);
            }
        }
    }

    if must_conditions.is_empty() && should_conditions.is_empty() && must_not_conditions.is_empty() {
        return None;
    }

    Some(Filter {
        must: must_conditions,
        should: should_conditions,
        must_not: must_not_conditions,
        min_should: None,
    })
}

/// Parse a single condition
fn parse_condition(condition: &Value) -> Option<Condition> {
    let obj = condition.as_object()?;
    let key = obj.get("key")?.as_str()?;
    let match_obj = obj.get("match")?.as_object()?;

    if let Some(value) = match_obj.get("value") {
        let match_value = match value {
            Value::String(s) => Match {
                match_value: Some(MatchValue::Keyword(s.clone())),
            },
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Match {
                        match_value: Some(MatchValue::Integer(i)),
                    }
                } else {
                    return None;
                }
            }
            Value::Bool(b) => Match {
                match_value: Some(MatchValue::Boolean(*b)),
            },
            _ => return None,
        };

        let field_condition = FieldCondition {
            key: key.to_string(),
            r#match: Some(match_value),
            range: None,
            geo_bounding_box: None,
            geo_radius: None,
            values_count: None,
            geo_polygon: None,
            datetime_range: None,
            is_empty: None,
            is_null: None,
        };

        Some(Condition {
            condition_one_of: Some(ConditionOneOf::Field(field_condition)),
        })
    } else {
        None
    }
}

/// Execute search operation
///
/// Supports two modes:
/// 1. Direct vector: provide "vector" field with pre-computed embeddings
/// 2. Auto-embedding: provide "text" field and embedding_provider will generate vector
///
/// Config:
/// - collection: target collection name
/// - vector: pre-computed vector (optional if text is provided)
/// - text: text to embed for search (optional if vector is provided)
/// - embedding_provider: provider for auto-embedding (default: dashscope)
/// - dimension: embedding dimension (optional)
/// - limit: max results (default: 10)
/// - score_threshold: minimum similarity score (optional)
/// - with_payload: include payload in results (default: true)
/// - with_vectors: include vectors in results (default: false)
/// - filter: Qdrant filter object (optional)
async fn execute_search(
    client: &Qdrant,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
    embedding_provider: Option<&dyn EmbeddingProvider>,
) -> Result<HashMap<String, Value>, EngineError> {
    let collection = get_required_string(connection, "collection", state)?;

    // Get vector: either from direct "vector" field or auto-embed from "text" field
    let vector = if let Ok(v) = parse_vector(connection, state) {
        v
    } else if let Some(text) = get_optional_string(connection, "text", state) {
        // Auto-embed the text
        let provider = embedding_provider
            .ok_or_else(|| EngineError::QdrantError("embedding_provider required for text search".to_string()))?;

        let dimension = get_optional_u64(connection, "dimension").map(|d| d as usize);
        let options = EmbeddingOptions {
            model: None,
            dimension,
        };

        let response = provider.embed(vec![text], options).await
            .map_err(|e| EngineError::QdrantError(format!("Embedding failed: {}", e)))?;

        response.embeddings.into_iter()
            .next()
            .map(|e| e.vector)
            .ok_or_else(|| EngineError::QdrantError("No embedding returned".to_string()))?
    } else {
        return Err(EngineError::MissingField("vector or text required for search".to_string()));
    };

    let limit = get_optional_u64(connection, "limit").unwrap_or(10) as u64;
    let score_threshold = connection.get("score_threshold")
        .and_then(|v| v.as_f64())
        .map(|f| f as f32);
    let with_payload = get_optional_bool(connection, "with_payload").unwrap_or(true);
    let with_vectors = get_optional_bool(connection, "with_vectors").unwrap_or(false);

    let mut search_builder = SearchPointsBuilder::new(&collection, vector, limit)
        .with_payload(with_payload)
        .with_vectors(with_vectors);

    if let Some(threshold) = score_threshold {
        search_builder = search_builder.score_threshold(threshold);
    }

    if let Some(filter) = parse_filter(connection) {
        search_builder = search_builder.filter(filter);
    }

    let response = client.search_points(search_builder).await
        .map_err(|e| EngineError::QdrantError(e.to_string()))?;

    let results: Vec<Value> = response.result.iter().map(|point| {
        let id = match &point.id {
            Some(PointId { point_id_options: Some(id) }) => {
                match id {
                    qdrant_client::qdrant::point_id::PointIdOptions::Num(n) => Value::Number((*n).into()),
                    qdrant_client::qdrant::point_id::PointIdOptions::Uuid(s) => Value::String(s.clone()),
                }
            }
            _ => Value::Null,
        };

        let payload: HashMap<String, Value> = point.payload.iter()
            .map(|(k, v)| (k.clone(), qdrant_value_to_json(v)))
            .collect();

        serde_json::json!({
            "id": id,
            "score": point.score,
            "payload": payload,
        })
    }).collect();

    let mut outputs = HashMap::new();
    outputs.insert("results".to_string(), Value::Array(results.clone()));
    outputs.insert("count".to_string(), Value::Number(results.len().into()));

    Ok(outputs)
}

/// Execute scroll operation
async fn execute_scroll(
    client: &Qdrant,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let collection = get_required_string(connection, "collection", state)?;
    let limit = get_optional_u64(connection, "limit").unwrap_or(100) as u32;
    let with_payload = get_optional_bool(connection, "with_payload").unwrap_or(true);
    let with_vectors = get_optional_bool(connection, "with_vectors").unwrap_or(false);

    let mut scroll_builder = ScrollPointsBuilder::new(&collection)
        .limit(limit)
        .with_payload(with_payload)
        .with_vectors(with_vectors);

    if let Some(filter) = parse_filter(connection) {
        scroll_builder = scroll_builder.filter(filter);
    }

    // Handle offset
    if let Some(offset_str) = get_optional_string(connection, "offset", state) {
        if !offset_str.is_empty() {
            // Try to parse as number or UUID
            if let Ok(num) = offset_str.parse::<u64>() {
                scroll_builder = scroll_builder.offset(PointId::from(num));
            } else {
                scroll_builder = scroll_builder.offset(PointId::from(offset_str));
            }
        }
    }

    let response = client.scroll(scroll_builder).await
        .map_err(|e| EngineError::QdrantError(e.to_string()))?;

    let points: Vec<Value> = response.result.iter().map(|point| {
        let id = match &point.id {
            Some(PointId { point_id_options: Some(id) }) => {
                match id {
                    qdrant_client::qdrant::point_id::PointIdOptions::Num(n) => Value::Number((*n).into()),
                    qdrant_client::qdrant::point_id::PointIdOptions::Uuid(s) => Value::String(s.clone()),
                }
            }
            _ => Value::Null,
        };

        let payload: HashMap<String, Value> = point.payload.iter()
            .map(|(k, v)| (k.clone(), qdrant_value_to_json(v)))
            .collect();

        serde_json::json!({
            "id": id,
            "payload": payload,
        })
    }).collect();

    let next_offset = response.next_page_offset.as_ref().and_then(|id| {
        id.point_id_options.as_ref().map(|opts| {
            match opts {
                qdrant_client::qdrant::point_id::PointIdOptions::Num(n) => Value::Number((*n).into()),
                qdrant_client::qdrant::point_id::PointIdOptions::Uuid(s) => Value::String(s.clone()),
            }
        })
    }).unwrap_or(Value::Null);

    let mut outputs = HashMap::new();
    outputs.insert("points".to_string(), Value::Array(points.clone()));
    outputs.insert("next_offset".to_string(), next_offset);
    outputs.insert("count".to_string(), Value::Number(points.len().into()));

    Ok(outputs)
}

/// Execute get_points operation
async fn execute_get_points(
    client: &Qdrant,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let collection = get_required_string(connection, "collection", state)?;
    let with_payload = get_optional_bool(connection, "with_payload").unwrap_or(true);
    let with_vectors = get_optional_bool(connection, "with_vectors").unwrap_or(false);

    // Parse IDs
    let ids_value = connection.get("ids")
        .ok_or_else(|| EngineError::MissingField("ids".to_string()))?;

    let ids: Vec<PointId> = match ids_value {
        Value::Array(arr) => {
            arr.iter()
                .filter_map(|v| {
                    match v {
                        Value::Number(n) => n.as_u64().map(PointId::from),
                        Value::String(s) => Some(PointId::from(s.clone())),
                        _ => None,
                    }
                })
                .collect()
        }
        _ => return Err(EngineError::MissingField("ids must be an array".to_string())),
    };

    let get_builder = GetPointsBuilder::new(&collection, ids)
        .with_payload(with_payload)
        .with_vectors(with_vectors);

    let response = client.get_points(get_builder).await
        .map_err(|e| EngineError::QdrantError(e.to_string()))?;

    let points: Vec<Value> = response.result.iter().map(|point| {
        let id = match &point.id {
            Some(PointId { point_id_options: Some(id) }) => {
                match id {
                    qdrant_client::qdrant::point_id::PointIdOptions::Num(n) => Value::Number((*n).into()),
                    qdrant_client::qdrant::point_id::PointIdOptions::Uuid(s) => Value::String(s.clone()),
                }
            }
            _ => Value::Null,
        };

        let payload: HashMap<String, Value> = point.payload.iter()
            .map(|(k, v)| (k.clone(), qdrant_value_to_json(v)))
            .collect();

        serde_json::json!({
            "id": id,
            "payload": payload,
        })
    }).collect();

    let mut outputs = HashMap::new();
    outputs.insert("points".to_string(), Value::Array(points.clone()));
    outputs.insert("count".to_string(), Value::Number(points.len().into()));

    Ok(outputs)
}

/// Convert Qdrant Value to serde_json Value
fn qdrant_value_to_json(value: &qdrant_client::qdrant::Value) -> Value {
    use qdrant_client::qdrant::value::Kind;

    match &value.kind {
        Some(Kind::NullValue(_)) => Value::Null,
        Some(Kind::BoolValue(b)) => Value::Bool(*b),
        Some(Kind::IntegerValue(i)) => Value::Number((*i).into()),
        Some(Kind::DoubleValue(d)) => {
            serde_json::Number::from_f64(*d)
                .map(Value::Number)
                .unwrap_or(Value::Null)
        }
        Some(Kind::StringValue(s)) => Value::String(s.clone()),
        Some(Kind::ListValue(list)) => {
            Value::Array(list.values.iter().map(qdrant_value_to_json).collect())
        }
        Some(Kind::StructValue(obj)) => {
            let map: serde_json::Map<String, Value> = obj.fields.iter()
                .map(|(k, v)| (k.clone(), qdrant_value_to_json(v)))
                .collect();
            Value::Object(map)
        }
        None => Value::Null,
    }
}

/// Convert serde_json Value to Qdrant Value
fn json_to_qdrant_value(value: &Value) -> qdrant_client::qdrant::Value {
    use qdrant_client::qdrant::{value::Kind, ListValue, Struct};

    let kind = match value {
        Value::Null => Kind::NullValue(0),
        Value::Bool(b) => Kind::BoolValue(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Kind::IntegerValue(i)
            } else if let Some(f) = n.as_f64() {
                Kind::DoubleValue(f)
            } else {
                Kind::NullValue(0)
            }
        }
        Value::String(s) => Kind::StringValue(s.clone()),
        Value::Array(arr) => Kind::ListValue(ListValue {
            values: arr.iter().map(json_to_qdrant_value).collect(),
        }),
        Value::Object(obj) => Kind::StructValue(Struct {
            fields: obj.iter()
                .map(|(k, v)| (k.clone(), json_to_qdrant_value(v)))
                .collect(),
        }),
    };

    qdrant_client::qdrant::Value { kind: Some(kind) }
}

/// Execute upsert operation
async fn execute_upsert(
    client: &Qdrant,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
    embedding_provider: Option<&dyn EmbeddingProvider>,
) -> Result<HashMap<String, Value>, EngineError> {
    let collection = get_required_string(connection, "collection", state)?;

    // Get points to upsert
    let points_value = connection.get("points")
        .ok_or_else(|| EngineError::MissingField("points".to_string()))?;

    let points_array = points_value.as_array()
        .ok_or_else(|| EngineError::QdrantError("points must be an array".to_string()))?;

    // Get embedding config if text field needs auto-embedding
    let text_field = get_optional_string(connection, "text_field", state);
    let dimension = get_optional_u64(connection, "dimension").map(|d| d as usize);

    // Collect texts for batch embedding
    let mut texts_to_embed: Vec<(usize, String)> = Vec::new();
    if let Some(ref field) = text_field {
        for (idx, point) in points_array.iter().enumerate() {
            if let Some(payload) = point.get("payload").and_then(|p| p.as_object()) {
                if let Some(text) = payload.get(field).and_then(|t| t.as_str()) {
                    texts_to_embed.push((idx, text.to_string()));
                }
            }
        }
    }

    // Generate embeddings if needed
    let mut embeddings: HashMap<usize, Vec<f32>> = HashMap::new();
    if !texts_to_embed.is_empty() {
        if let Some(provider) = embedding_provider {
            let texts: Vec<String> = texts_to_embed.iter().map(|(_, t)| t.clone()).collect();
            let options = EmbeddingOptions {
                model: None,
                dimension,
            };

            let response = provider.embed(texts, options).await
                .map_err(|e| EngineError::QdrantError(format!("Embedding failed: {}", e)))?;

            for (i, (idx, _)) in texts_to_embed.iter().enumerate() {
                if let Some(embedding) = response.embeddings.get(i) {
                    embeddings.insert(*idx, embedding.vector.clone());
                }
            }
        }
    }

    // Build point structs
    let mut point_structs: Vec<PointStruct> = Vec::new();
    for (idx, point) in points_array.iter().enumerate() {
        // Parse ID
        let id = point.get("id")
            .ok_or_else(|| EngineError::QdrantError("Point missing id".to_string()))?;

        let point_id: PointId = match id {
            Value::Number(n) => {
                let num = n.as_u64()
                    .ok_or_else(|| EngineError::QdrantError("Invalid numeric id".to_string()))?;
                PointId::from(num)
            }
            Value::String(s) => PointId::from(s.clone()),
            _ => return Err(EngineError::QdrantError("Invalid id type".to_string())),
        };

        // Get vector - either from point or from embedding
        let vector: Vec<f32> = if let Some(v) = embeddings.get(&idx) {
            v.clone()
        } else if let Some(vector_value) = point.get("vector") {
            vector_value.as_array()
                .ok_or_else(|| EngineError::QdrantError("vector must be an array".to_string()))?
                .iter()
                .map(|v| v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| EngineError::QdrantError("Invalid vector value".to_string())))
                .collect::<Result<Vec<f32>, _>>()?
        } else {
            return Err(EngineError::QdrantError("Point missing vector".to_string()));
        };

        // Parse payload
        let payload: HashMap<String, qdrant_client::qdrant::Value> = point.get("payload")
            .and_then(|p| p.as_object())
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), json_to_qdrant_value(v)))
                    .collect()
            })
            .unwrap_or_default();

        point_structs.push(PointStruct::new(point_id, vector, payload));
    }

    // Execute upsert
    let upsert_builder = UpsertPointsBuilder::new(&collection, point_structs);

    client.upsert_points(upsert_builder).await
        .map_err(|e| EngineError::QdrantError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("success".to_string(), Value::Bool(true));
    outputs.insert("count".to_string(), Value::Number(points_array.len().into()));

    Ok(outputs)
}

/// Execute batch_upsert operation
///
/// Takes an array of texts, embeds each one, and stores them as separate points
/// with shared payload. Used for storing sentences with their full text context.
///
/// Config:
/// - collection: target collection name
/// - texts: array of strings to embed (resolved from state path or literal)
/// - text_field_name: field name for storing individual text in payload
/// - shared_payload: additional payload fields shared by all points
/// - embedding_provider: provider to use for embeddings (default: dashscope)
/// - dimension: embedding dimension (optional)
async fn execute_batch_upsert(
    client: &Qdrant,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
    embedding_provider: Option<&dyn EmbeddingProvider>,
) -> Result<HashMap<String, Value>, EngineError> {
    let collection = get_required_string(connection, "collection", state)?;

    // Get text field name (defaults to "text")
    let text_field_name = get_optional_string(connection, "text_field_name", state)
        .unwrap_or_else(|| "text".to_string());

    // Get dimension for embedding
    let dimension = get_optional_u64(connection, "dimension").map(|d| d as usize);

    // Get texts to embed - resolve from state path or literal array
    let texts: Vec<String> = {
        let texts_value = connection.get("texts")
            .ok_or_else(|| EngineError::MissingField("texts".to_string()))?;

        match texts_value {
            Value::Array(arr) => {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            }
            Value::String(path) => {
                // Resolve from state
                let resolved = super::resolve_template_vars(path, state);
                state.read(&resolved)
                    .and_then(|v| {
                        v.as_array().map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                    })
                    .ok_or_else(|| EngineError::QdrantError(format!("texts not found in state at path: {}", resolved)))?
            }
            _ => return Err(EngineError::QdrantError("texts must be an array or state path".to_string())),
        }
    };

    if texts.is_empty() {
        let mut outputs = HashMap::new();
        outputs.insert("success".to_string(), Value::Bool(true));
        outputs.insert("count".to_string(), Value::Number(0.into()));
        outputs.insert("point_ids".to_string(), Value::Array(vec![]));
        return Ok(outputs);
    }

    // Get shared payload
    let shared_payload: HashMap<String, Value> = connection.get("shared_payload")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| {
                    // Resolve any template variables in payload values
                    let resolved_value = match v {
                        Value::String(s) => {
                            let resolved = super::resolve_template_vars(s, state);
                            // Try to get from state, if not found use the resolved string
                            state.read(&resolved)
                                .unwrap_or_else(|| Value::String(resolved))
                        }
                        _ => v.clone(),
                    };
                    (k.clone(), resolved_value)
                })
                .collect()
        })
        .unwrap_or_default();

    // Generate embeddings for all texts
    let embeddings = if let Some(provider) = embedding_provider {
        let options = EmbeddingOptions {
            model: None,
            dimension,
        };

        let response = provider.embed(texts.clone(), options).await
            .map_err(|e| EngineError::QdrantError(format!("Embedding failed: {}", e)))?;

        response.embeddings.into_iter()
            .map(|e| e.vector)
            .collect::<Vec<Vec<f32>>>()
    } else {
        return Err(EngineError::QdrantError("embedding_provider required for batch_upsert".to_string()));
    };

    if embeddings.len() != texts.len() {
        return Err(EngineError::QdrantError(format!(
            "Embedding count mismatch: got {} embeddings for {} texts",
            embeddings.len(),
            texts.len()
        )));
    }

    // Build point structs
    let mut point_structs: Vec<PointStruct> = Vec::new();
    let mut point_ids: Vec<Value> = Vec::new();

    for (text, vector) in texts.iter().zip(embeddings.into_iter()) {
        // Generate UUID for each point
        let uuid = uuid::Uuid::new_v4().to_string();
        let point_id = PointId::from(uuid.clone());
        point_ids.push(Value::String(uuid));

        // Build payload: text_field_name -> text, plus shared_payload
        let mut payload: HashMap<String, qdrant_client::qdrant::Value> = HashMap::new();

        // Add the individual text
        payload.insert(
            text_field_name.clone(),
            json_to_qdrant_value(&Value::String(text.clone())),
        );

        // Add shared payload fields
        for (key, value) in &shared_payload {
            payload.insert(key.to_string(), json_to_qdrant_value(value));
        }

        point_structs.push(PointStruct::new(point_id, vector, payload));
    }

    // Execute upsert
    let upsert_builder = UpsertPointsBuilder::new(&collection, point_structs);

    client.upsert_points(upsert_builder).await
        .map_err(|e| EngineError::QdrantError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("success".to_string(), Value::Bool(true));
    outputs.insert("count".to_string(), Value::Number(texts.len().into()));
    outputs.insert("point_ids".to_string(), Value::Array(point_ids));

    Ok(outputs)
}

/// Execute delete operation
async fn execute_delete(
    client: &Qdrant,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let collection = get_required_string(connection, "collection", state)?;

    // Get points selector - either IDs or filter
    let delete_builder = if let Some(ids_value) = connection.get("ids") {
        let ids: Vec<PointId> = ids_value.as_array()
            .ok_or_else(|| EngineError::QdrantError("ids must be an array".to_string()))?
            .iter()
            .filter_map(|v| match v {
                Value::Number(n) => n.as_u64().map(PointId::from),
                Value::String(s) => Some(PointId::from(s.clone())),
                _ => None,
            })
            .collect();

        DeletePointsBuilder::new(&collection)
            .points(PointsIdsList { ids })
    } else if let Some(filter) = parse_filter(connection) {
        DeletePointsBuilder::new(&collection)
            .points(filter)
    } else {
        return Err(EngineError::MissingField("ids or filter required for delete".to_string()));
    };

    // Execute delete
    client.delete_points(delete_builder).await
        .map_err(|e| EngineError::QdrantError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("success".to_string(), Value::Bool(true));

    Ok(outputs)
}

/// Execute create_collection operation
async fn execute_create_collection(
    client: &Qdrant,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let collection = get_required_string(connection, "collection", state)?;

    // Get vector configuration
    let dimension = get_optional_u64(connection, "dimension")
        .ok_or_else(|| EngineError::MissingField("dimension".to_string()))? as u64;

    let distance_str = get_optional_string(connection, "distance", state)
        .unwrap_or_else(|| "Cosine".to_string());

    let distance = match distance_str.to_lowercase().as_str() {
        "cosine" => Distance::Cosine,
        "euclid" | "euclidean" => Distance::Euclid,
        "dot" | "dotproduct" => Distance::Dot,
        "manhattan" => Distance::Manhattan,
        _ => return Err(EngineError::QdrantError(format!("Unknown distance: {}", distance_str))),
    };

    // Build vector params
    let vector_params = VectorParamsBuilder::new(dimension, distance);

    // Create collection
    let create_builder = CreateCollectionBuilder::new(&collection)
        .vectors_config(vector_params);

    client.create_collection(create_builder).await
        .map_err(|e| EngineError::QdrantError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("success".to_string(), Value::Bool(true));
    outputs.insert("collection".to_string(), Value::String(collection));

    Ok(outputs)
}

