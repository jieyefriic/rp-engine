//! Qdrant vector database connector implementation
//!
//! Supports operations: search, scroll, get_points

use std::collections::HashMap;
use qdrant_client::Qdrant;
use qdrant_client::qdrant::{
    SearchPointsBuilder, ScrollPointsBuilder, GetPointsBuilder,
    Filter, Condition, PointId,
    condition::ConditionOneOf, FieldCondition, r#match::MatchValue, Match,
};
use serde_json::Value;

use crate::schema::NodeConfig;
use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, get_optional_bool};

/// Execute a Qdrant operation
pub async fn execute(
    config: &NodeConfig,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let connection = &config.connection;

    // Get connection parameters
    let url = get_required_string(connection, "url", state)?;
    let api_key = get_optional_string(connection, "api_key", state);

    // Get operation
    let operation = get_required_string(connection, "operation", state)?;

    // Create Qdrant client
    let client = create_qdrant_client(&url, api_key.as_deref()).await?;

    match operation.as_str() {
        "search" => execute_search(&client, connection, state).await,
        "scroll" => execute_scroll(&client, connection, state).await,
        "get_points" => execute_get_points(&client, connection, state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown Qdrant operation: {}", operation))),
    }
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
async fn execute_search(
    client: &Qdrant,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let collection = get_required_string(connection, "collection", state)?;
    let vector = parse_vector(connection, state)?;
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

/// Execute Qdrant with V2 config (data source + operation)
pub async fn execute_v2(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get connection parameters from data source config
    let url = get_required_string(ds_config, "url", state)?;
    let api_key = get_optional_string(ds_config, "api_key", state);

    // Create Qdrant client
    let client = create_qdrant_client(&url, api_key.as_deref()).await?;

    // Get operation type
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("operation".to_string()))?;

    match op_type {
        "search" => execute_search(&client, operation, state).await,
        "scroll" => execute_scroll(&client, operation, state).await,
        "get_points" => execute_get_points(&client, operation, state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown Qdrant operation: {}", op_type))),
    }
}
