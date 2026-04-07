//! MongoDB connector implementation
//!
//! Supports operations: find, find_one, insert_one, insert_many, update_one, update_many,
//! delete_one, delete_many, aggregate, count

use std::collections::HashMap;
use mongodb::{Client, options::ClientOptions};
use mongodb::bson::{doc, Document, Bson};
use futures::TryStreamExt;
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, ConnectorContext};

/// Unified MongoDB execute function
pub async fn execute(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    ctx: &ConnectorContext<'_>,
) -> Result<HashMap<String, Value>, EngineError> {
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("operation".to_string()))?;

    debug!(operation = %op_type, "Starting MongoDB operation");

    let (client, db_name) = create_client(ds_config, ctx.state).await?;
    let database = get_optional_string(operation, "database", ctx.state)
        .unwrap_or(db_name);
    let collection_name = get_required_string(operation, "collection", ctx.state)?;

    let db = client.database(&database);
    let collection = db.collection::<Document>(&collection_name);

    let result = match op_type {
        "find" => execute_find(&collection, operation, ctx.variables).await,
        "find_one" => execute_find_one(&collection, operation, ctx.variables).await,
        "insert_one" => execute_insert_one(&collection, operation, ctx.variables).await,
        "insert_many" => execute_insert_many(&collection, operation, ctx.variables).await,
        "update_one" => execute_update_one(&collection, operation, ctx.variables).await,
        "update_many" => execute_update_many(&collection, operation, ctx.variables).await,
        "delete_one" => execute_delete_one(&collection, operation, ctx.variables).await,
        "delete_many" => execute_delete_many(&collection, operation, ctx.variables).await,
        "aggregate" => execute_aggregate(&collection, operation, ctx.variables).await,
        "count" => execute_count(&collection, operation, ctx.variables).await,
        _ => Err(EngineError::InvalidOperation(format!(
            "Unknown MongoDB operation: {}. Available: find, find_one, insert_one, insert_many, \
             update_one, update_many, delete_one, delete_many, aggregate, count", op_type
        ))),
    };

    result
}

/// Create MongoDB client
async fn create_client(
    ds_config: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<(Client, String), EngineError> {
    let url = get_required_string(ds_config, "url", state)?;
    let database = get_optional_string(ds_config, "database", state)
        .unwrap_or_else(|| "test".to_string());

    let client_options = ClientOptions::parse(&url).await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB connection failed: {}", e)))?;

    let client = Client::with_options(client_options)
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB client creation failed: {}", e)))?;

    Ok((client, database))
}

/// Execute find (query multiple documents)
async fn execute_find(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let filter = build_filter(operation, variables);
    let limit = get_optional_u64(operation, "limit").map(|l| l as i64);
    let skip = get_optional_u64(operation, "skip").map(|s| s as u64);

    let mut find_opts = mongodb::options::FindOptions::default();
    find_opts.limit = limit;
    find_opts.skip = skip;

    // Sort
    if let Some(sort_val) = operation.get("sort") {
        if let Ok(sort_doc) = json_to_bson_document(sort_val) {
            find_opts.sort = Some(sort_doc);
        }
    }

    // Projection
    if let Some(proj_val) = operation.get("projection") {
        if let Ok(proj_doc) = json_to_bson_document(proj_val) {
            find_opts.projection = Some(proj_doc);
        }
    }

    debug!(filter = ?filter, "Executing MongoDB find");

    let mut cursor = collection.find(filter)
        .with_options(find_opts)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB find failed: {}", e)))?;

    let mut docs = Vec::new();
    const MAX_DOCS: usize = 10_000;

    while let Some(doc) = cursor.try_next().await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB cursor error: {}", e)))?
    {
        docs.push(bson_doc_to_json(&doc));
        if docs.len() >= MAX_DOCS {
            warn!(limit = MAX_DOCS, "MongoDB find result truncated");
            break;
        }
    }

    debug!(count = docs.len(), "MongoDB find completed");

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(docs.clone()));
    outputs.insert("count".to_string(), Value::Number(docs.len().into()));

    // Flatten first document
    if let Some(Value::Object(obj)) = docs.first() {
        for (key, value) in obj {
            outputs.insert(key.clone(), value.clone());
        }
    }

    Ok(outputs)
}

/// Execute find_one
async fn execute_find_one(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let filter = build_filter(operation, variables);

    debug!(filter = ?filter, "Executing MongoDB find_one");

    let doc = collection.find_one(filter)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB find_one failed: {}", e)))?;

    let mut outputs = HashMap::new();

    if let Some(doc) = doc {
        let json_doc = bson_doc_to_json(&doc);
        outputs.insert("exists".to_string(), Value::Bool(true));
        outputs.insert("document".to_string(), json_doc.clone());

        // Flatten fields
        if let Value::Object(obj) = &json_doc {
            for (key, value) in obj {
                outputs.insert(key.clone(), value.clone());
            }
        }
    } else {
        outputs.insert("exists".to_string(), Value::Bool(false));
        outputs.insert("document".to_string(), Value::Null);
    }

    Ok(outputs)
}

/// Execute insert_one
async fn execute_insert_one(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let data = operation.get("data")
        .ok_or_else(|| EngineError::MissingField("data".to_string()))?;

    let resolved = resolve_json_value(data, variables);
    let doc = json_to_bson_document(&resolved)
        .map_err(|e| EngineError::DatabaseError(format!("Failed to convert data to BSON: {}", e)))?;

    debug!("Executing MongoDB insert_one");

    let result = collection.insert_one(doc)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB insert_one failed: {}", e)))?;

    let inserted_id = bson_to_json(&Bson::from(result.inserted_id));

    let mut outputs = HashMap::new();
    outputs.insert("inserted_id".to_string(), inserted_id);
    outputs.insert("count".to_string(), Value::Number(1.into()));

    Ok(outputs)
}

/// Execute insert_many
async fn execute_insert_many(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let data = operation.get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| EngineError::MissingField("data (array)".to_string()))?;

    let docs: Vec<Document> = data.iter()
        .map(|item| {
            let resolved = resolve_json_value(item, variables);
            json_to_bson_document(&resolved)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| EngineError::DatabaseError(format!("Failed to convert data to BSON: {}", e)))?;

    let count = docs.len();
    debug!(count = count, "Executing MongoDB insert_many");

    let result = collection.insert_many(docs)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB insert_many failed: {}", e)))?;

    let ids: Vec<Value> = result.inserted_ids.values()
        .map(|id| bson_to_json(id))
        .collect();

    let mut outputs = HashMap::new();
    outputs.insert("inserted_ids".to_string(), Value::Array(ids));
    outputs.insert("count".to_string(), Value::Number(count.into()));

    Ok(outputs)
}

/// Execute update_one
async fn execute_update_one(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let filter = build_filter(operation, variables);
    let update = build_update(operation, variables)?;

    debug!(filter = ?filter, "Executing MongoDB update_one");

    let result = collection.update_one(filter, update)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB update_one failed: {}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("matched_count".to_string(), Value::Number(result.matched_count.into()));
    outputs.insert("modified_count".to_string(), Value::Number(result.modified_count.into()));
    outputs.insert("count".to_string(), Value::Number(result.modified_count.into()));

    Ok(outputs)
}

/// Execute update_many
async fn execute_update_many(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let filter = build_filter(operation, variables);
    let update = build_update(operation, variables)?;

    debug!(filter = ?filter, "Executing MongoDB update_many");

    let result = collection.update_many(filter, update)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB update_many failed: {}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("matched_count".to_string(), Value::Number(result.matched_count.into()));
    outputs.insert("modified_count".to_string(), Value::Number(result.modified_count.into()));
    outputs.insert("count".to_string(), Value::Number(result.modified_count.into()));

    Ok(outputs)
}

/// Execute delete_one
async fn execute_delete_one(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let filter = build_filter(operation, variables);

    debug!(filter = ?filter, "Executing MongoDB delete_one");

    let result = collection.delete_one(filter)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB delete_one failed: {}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("deleted_count".to_string(), Value::Number(result.deleted_count.into()));
    outputs.insert("count".to_string(), Value::Number(result.deleted_count.into()));

    Ok(outputs)
}

/// Execute delete_many
async fn execute_delete_many(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let filter = build_filter(operation, variables);

    debug!(filter = ?filter, "Executing MongoDB delete_many");

    let result = collection.delete_many(filter)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB delete_many failed: {}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("deleted_count".to_string(), Value::Number(result.deleted_count.into()));
    outputs.insert("count".to_string(), Value::Number(result.deleted_count.into()));

    Ok(outputs)
}

/// Execute aggregate pipeline
async fn execute_aggregate(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let pipeline_val = operation.get("pipeline")
        .and_then(|v| v.as_array())
        .ok_or_else(|| EngineError::MissingField("pipeline (array)".to_string()))?;

    let pipeline: Vec<Document> = pipeline_val.iter()
        .map(|stage| {
            let resolved = resolve_json_value(stage, variables);
            json_to_bson_document(&resolved)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| EngineError::DatabaseError(format!("Invalid pipeline: {}", e)))?;

    debug!(stages = pipeline.len(), "Executing MongoDB aggregate");

    let mut cursor = collection.aggregate(pipeline)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB aggregate failed: {}", e)))?;

    let mut docs = Vec::new();
    const MAX_DOCS: usize = 10_000;

    while let Some(doc) = cursor.try_next().await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB cursor error: {}", e)))?
    {
        docs.push(bson_doc_to_json(&doc));
        if docs.len() >= MAX_DOCS {
            warn!(limit = MAX_DOCS, "MongoDB aggregate result truncated");
            break;
        }
    }

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(docs.clone()));
    outputs.insert("count".to_string(), Value::Number(docs.len().into()));

    if let Some(Value::Object(obj)) = docs.first() {
        for (key, value) in obj {
            outputs.insert(key.clone(), value.clone());
        }
    }

    Ok(outputs)
}

/// Execute count
async fn execute_count(
    collection: &mongodb::Collection<Document>,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, EngineError> {
    let filter = build_filter(operation, variables);

    debug!(filter = ?filter, "Executing MongoDB count");

    let count = collection.count_documents(filter)
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MongoDB count failed: {}", e)))?;

    let mut outputs = HashMap::new();
    outputs.insert("count".to_string(), Value::Number(count.into()));

    Ok(outputs)
}

// ============================================================================
// Helpers
// ============================================================================

/// Build filter document from operation config
fn build_filter(
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Document {
    operation.get("filter")
        .map(|v| {
            let resolved = resolve_json_value(v, variables);
            json_to_bson_document(&resolved).unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Build update document from operation config
fn build_update(
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> Result<Document, EngineError> {
    let update_val = operation.get("update")
        .ok_or_else(|| EngineError::MissingField("update".to_string()))?;

    let resolved = resolve_json_value(update_val, variables);
    let update_doc = json_to_bson_document(&resolved)
        .map_err(|e| EngineError::DatabaseError(format!("Invalid update document: {}", e)))?;

    // If update doesn't start with $ operator, wrap in $set
    let has_operator = update_doc.keys().any(|k| k.starts_with('$'));
    if has_operator {
        Ok(update_doc)
    } else {
        Ok(doc! { "$set": update_doc })
    }
}

/// Resolve {paramName} references in JSON values
fn resolve_json_value(val: &Value, variables: &HashMap<String, Value>) -> Value {
    match val {
        Value::String(s) => {
            if s.starts_with('{') && s.ends_with('}') && !s.contains(' ') {
                let name = &s[1..s.len()-1];
                variables.get(name).cloned().unwrap_or(Value::Null)
            } else {
                val.clone()
            }
        }
        Value::Object(obj) => {
            let resolved: serde_json::Map<String, Value> = obj.iter()
                .map(|(k, v)| (k.clone(), resolve_json_value(v, variables)))
                .collect();
            Value::Object(resolved)
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| resolve_json_value(v, variables)).collect())
        }
        _ => val.clone(),
    }
}

/// Convert JSON Value to BSON Document
fn json_to_bson_document(val: &Value) -> Result<Document, String> {
    let bson = json_to_bson(val);
    match bson {
        Bson::Document(doc) => Ok(doc),
        _ => Err("Expected object, got non-document BSON".to_string()),
    }
}

/// Convert JSON Value to BSON
fn json_to_bson(val: &Value) -> Bson {
    match val {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Bson::Int64(i)
            } else if let Some(f) = n.as_f64() {
                Bson::Double(f)
            } else {
                Bson::Null
            }
        }
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(arr) => {
            Bson::Array(arr.iter().map(json_to_bson).collect())
        }
        Value::Object(obj) => {
            let doc: Document = obj.iter()
                .map(|(k, v)| (k.clone(), json_to_bson(v)))
                .collect();
            Bson::Document(doc)
        }
    }
}

/// Convert BSON Document to JSON Value
fn bson_doc_to_json(doc: &Document) -> Value {
    let mut obj = serde_json::Map::new();
    for (key, value) in doc {
        obj.insert(key.clone(), bson_to_json(value));
    }
    Value::Object(obj)
}

/// Convert BSON to JSON Value
fn bson_to_json(bson: &Bson) -> Value {
    match bson {
        Bson::Null => Value::Null,
        Bson::Boolean(b) => Value::Bool(*b),
        Bson::Int32(i) => Value::Number((*i as i64).into()),
        Bson::Int64(i) => Value::Number((*i).into()),
        Bson::Double(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Bson::String(s) => Value::String(s.clone()),
        Bson::Array(arr) => Value::Array(arr.iter().map(bson_to_json).collect()),
        Bson::Document(doc) => bson_doc_to_json(doc),
        Bson::ObjectId(oid) => Value::String(oid.to_hex()),
        Bson::DateTime(dt) => Value::String(dt.to_string()),
        Bson::Timestamp(ts) => json!({"t": ts.time, "i": ts.increment}),
        Bson::Binary(bin) => Value::String(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD, &bin.bytes)),
        Bson::RegularExpression(regex) => json!({"pattern": regex.pattern, "options": regex.options}),
        Bson::Decimal128(d) => Value::String(d.to_string()),
        _ => Value::String(format!("{:?}", bson)),
    }
}
