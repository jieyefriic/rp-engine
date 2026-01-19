//! PostgreSQL data connector implementation
//!
//! Supports operations: query, insert, select, update, delete
//!
//! Supports both:
//! - Legacy: `params` array with `{{node.field}}` syntax
//! - New: `bindings` map with `{paramName}` syntax in SQL

use std::collections::HashMap;
use tokio_postgres::{NoTls, Row, types::ToSql};
use serde_json::Value;

use crate::schema::NodeConfig;
use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, resolve_template_vars};

/// Resolve {paramName} patterns in a SQL template using bindings
///
/// For example, with:
/// - sql: "SELECT * FROM users WHERE id = {userId}"
/// - bindings: {"userId": "start.user_id"}
/// - state contains: {"start.user_id": "123"}
///
/// Returns: "SELECT * FROM users WHERE id = 123"
fn resolve_sql_with_bindings(
    sql: &str,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> String {
    let re = regex_lite::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();

    re.replace_all(sql, |caps: &regex_lite::Captures| {
        let param_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");

        // Look up the binding for this parameter
        if let Some(state_path) = bindings.get(param_name) {
            // Resolve the state path to get the actual value
            state.read(state_path)
                .map(|v| match v {
                    Value::String(s) => format!("'{}'", s.replace('\'', "''")), // Escape single quotes
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "NULL".to_string(),
                    other => format!("'{}'", other.to_string().replace('\'', "''")),
                })
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            // No binding found, leave the placeholder as-is
            format!("{{{}}}", param_name)
        }
    }).to_string()
}

/// Execute a PostgreSQL operation
pub async fn execute(
    config: &NodeConfig,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let connection_config = &config.connection;

    // Get operation
    let operation = get_required_string(connection_config, "operation", state)?;

    // Build connection string
    let conn_string = get_connection_string(connection_config, state)?;

    // Connect to database
    let (client, db_connection) = tokio_postgres::connect(&conn_string, NoTls)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    // Spawn connection task
    tokio::spawn(async move {
        if let Err(e) = db_connection.await {
            tracing::error!("PostgreSQL connection error: {}", e);
        }
    });

    match operation.as_str() {
        "query" => execute_query(&client, connection_config, state).await,
        "insert" => execute_insert(&client, connection_config, state).await,
        "select" => execute_select(&client, connection_config, state).await,
        "update" => execute_update(&client, connection_config, state).await,
        "delete" => execute_delete(&client, connection_config, state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown PostgreSQL operation: {}", operation))),
    }
}

/// Build PostgreSQL connection string
fn get_connection_string(
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    // Try direct connection string first
    if let Some(url) = get_optional_string(connection, "url", state) {
        return Ok(url);
    }

    // Build from components
    let host = get_optional_string(connection, "host", state)
        .unwrap_or_else(|| "localhost".to_string());
    let port = get_optional_u64(connection, "port").unwrap_or(5432);
    let username = get_required_string(connection, "username", state)?;
    let password = get_required_string(connection, "password", state)?;
    let database = get_required_string(connection, "database", state)?;

    Ok(format!(
        "host={} port={} user={} password={} dbname={}",
        host, port, username, password, database
    ))
}

/// Execute raw SQL query
async fn execute_query(
    client: &tokio_postgres::Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let sql = get_required_string(connection, "sql", state)?;

    // Parse parameters
    let params = parse_query_params(connection, state)?;
    let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect();

    let rows = client.query(&sql, &param_refs)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    Ok(outputs)
}

/// Execute INSERT operation (CRUD)
async fn execute_insert(
    client: &tokio_postgres::Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(connection, "table", state)?;

    // Get data to insert
    let data = connection.get("data")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::MissingField("data".to_string()))?;

    // Build column names and placeholders
    let mut columns: Vec<String> = Vec::new();
    let mut placeholders: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();

    for (i, (col, val)) in data.iter().enumerate() {
        columns.push(format!("\"{}\"", col));
        placeholders.push(format!("${}", i + 1));
        let resolved = match val {
            Value::String(s) => resolve_template_vars(s, state),
            other => other.to_string(),
        };
        values.push(resolved);
    }

    // Build RETURNING clause
    let returning = connection.get("returning")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let cols: Vec<String> = arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s))
                .collect();
            format!(" RETURNING {}", cols.join(", "))
        })
        .unwrap_or_default();

    let sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({}){}",
        table,
        columns.join(", "),
        placeholders.join(", "),
        returning
    );

    let params: Vec<&(dyn ToSql + Sync)> = values.iter()
        .map(|v| v as &(dyn ToSql + Sync))
        .collect();

    let rows = client.query(&sql, &params)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().max(1).into()));

    Ok(outputs)
}

/// Execute SELECT operation (CRUD)
async fn execute_select(
    client: &tokio_postgres::Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(connection, "table", state)?;

    // Get columns (default to *)
    let columns = connection.get("columns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "*".to_string());

    // Build WHERE clause
    let (where_clause, params) = build_where_clause(connection, state)?;

    // Get ORDER BY
    let order_by = get_optional_string(connection, "order_by", state)
        .map(|o| format!(" ORDER BY {}", o))
        .unwrap_or_default();

    // Get LIMIT
    let limit = get_optional_u64(connection, "limit")
        .map(|l| format!(" LIMIT {}", l))
        .unwrap_or_default();

    // Get OFFSET
    let offset = get_optional_u64(connection, "offset")
        .map(|o| format!(" OFFSET {}", o))
        .unwrap_or_default();

    let sql = format!(
        "SELECT {} FROM \"{}\"{}{}{}{}",
        columns, table, where_clause, order_by, limit, offset
    );

    let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect();

    let rows = client.query(&sql, &param_refs)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    Ok(outputs)
}

/// Execute UPDATE operation (CRUD)
async fn execute_update(
    client: &tokio_postgres::Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(connection, "table", state)?;

    // Get SET data
    let set_data = connection.get("set")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::MissingField("set".to_string()))?;

    // Build SET clause
    let mut set_parts: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    for (col, val) in set_data.iter() {
        let value_str = match val {
            Value::String(s) => {
                let resolved = resolve_template_vars(s, state);
                // Check if it's a SQL function like NOW()
                if resolved.to_uppercase().contains("NOW()") ||
                   resolved.to_uppercase().contains("CURRENT_TIMESTAMP") {
                    set_parts.push(format!("\"{}\" = {}", col, resolved));
                    continue;
                }
                resolved
            }
            other => other.to_string(),
        };
        set_parts.push(format!("\"{}\" = ${}", col, params.len() + 1));
        params.push(value_str);
    }

    // Build WHERE clause
    let (where_clause, where_params) = build_where_clause_with_offset(connection, state, params.len())?;
    params.extend(where_params);

    // Build RETURNING clause
    let returning = connection.get("returning")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let cols: Vec<String> = arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s))
                .collect();
            format!(" RETURNING {}", cols.join(", "))
        })
        .unwrap_or_default();

    let sql = format!(
        "UPDATE \"{}\" SET {}{}{}",
        table,
        set_parts.join(", "),
        where_clause,
        returning
    );

    let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect();

    let rows = client.query(&sql, &param_refs)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    Ok(outputs)
}

/// Execute DELETE operation (CRUD)
async fn execute_delete(
    client: &tokio_postgres::Client,
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(connection, "table", state)?;

    // Build WHERE clause (required for safety)
    let (where_clause, params) = build_where_clause(connection, state)?;

    if where_clause.is_empty() {
        return Err(EngineError::DatabaseError("DELETE requires WHERE clause for safety".to_string()));
    }

    let sql = format!("DELETE FROM \"{}\"{}",table, where_clause);

    let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect();

    let result = client.execute(&sql, &param_refs)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("count".to_string(), Value::Number(result.into()));

    Ok(outputs)
}

/// Parse query parameters from config
fn parse_query_params(
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<Vec<String>, EngineError> {
    let params = connection.get("params")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| match v {
                    Value::String(s) => resolve_template_vars(s, state),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(params)
}

/// Build WHERE clause from config
fn build_where_clause(
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<(String, Vec<String>), EngineError> {
    build_where_clause_with_offset(connection, state, 0)
}

/// Build WHERE clause with parameter offset
fn build_where_clause_with_offset(
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
    offset: usize,
) -> Result<(String, Vec<String>), EngineError> {
    let where_obj = match connection.get("where").and_then(|v| v.as_object()) {
        Some(obj) => obj,
        None => return Ok((String::new(), Vec::new())),
    };

    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    for (col, val) in where_obj {
        let value_str = match val {
            Value::String(s) => resolve_template_vars(s, state),
            other => other.to_string(),
        };
        conditions.push(format!("\"{}\" = ${}", col, offset + params.len() + 1));
        params.push(value_str);
    }

    if conditions.is_empty() {
        Ok((String::new(), Vec::new()))
    } else {
        Ok((format!(" WHERE {}", conditions.join(" AND ")), params))
    }
}

/// Convert PostgreSQL rows to JSON
fn rows_to_json(rows: &[Row]) -> Vec<Value> {
    rows.iter().map(|row| {
        let mut obj = serde_json::Map::new();
        for (i, column) in row.columns().iter().enumerate() {
            let name = column.name().to_string();
            let value = row_value_to_json(row, i, column.type_());
            obj.insert(name, value);
        }
        Value::Object(obj)
    }).collect()
}

/// Convert a single row value to JSON
fn row_value_to_json(row: &Row, idx: usize, pg_type: &tokio_postgres::types::Type) -> Value {
    use tokio_postgres::types::Type;

    match *pg_type {
        Type::BOOL => row.get::<_, Option<bool>>(idx)
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        Type::INT2 => row.get::<_, Option<i16>>(idx)
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),
        Type::INT4 => row.get::<_, Option<i32>>(idx)
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),
        Type::INT8 => row.get::<_, Option<i64>>(idx)
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),
        Type::FLOAT4 => row.get::<_, Option<f32>>(idx)
            .and_then(|v| serde_json::Number::from_f64(v as f64))
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Type::FLOAT8 => row.get::<_, Option<f64>>(idx)
            .and_then(|v| serde_json::Number::from_f64(v))
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Type::JSON | Type::JSONB => row.get::<_, Option<Value>>(idx)
            .unwrap_or(Value::Null),
        Type::UUID => row.get::<_, Option<uuid::Uuid>>(idx)
            .map(|v| Value::String(v.to_string()))
            .unwrap_or(Value::Null),
        _ => row.get::<_, Option<String>>(idx)
            .map(Value::String)
            .unwrap_or(Value::Null),
    }
}

/// Execute PostgreSQL with V2 config (data source + operation)
pub async fn execute_v2(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    // Build connection string from data source config
    let conn_string = get_connection_string(ds_config, state)?;

    // Connect to database
    let (client, db_connection) = tokio_postgres::connect(&conn_string, NoTls)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    // Spawn connection task
    tokio::spawn(async move {
        if let Err(e) = db_connection.await {
            tracing::error!("PostgreSQL connection error: {}", e);
        }
    });

    // Get operation type
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("operation".to_string()))?;

    match op_type {
        "query" => execute_query(&client, operation, state).await,
        "insert" => execute_insert(&client, operation, state).await,
        "select" => execute_select(&client, operation, state).await,
        "update" => execute_update(&client, operation, state).await,
        "delete" => execute_delete(&client, operation, state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown PostgreSQL operation: {}", op_type))),
    }
}

/// Execute PostgreSQL with connection pool (preferred for production)
///
/// Uses a connection from the pool instead of creating a new connection each time.
/// This provides better performance and resource management.
///
/// Supports `bindings` for `{paramName}` syntax in SQL queries.
pub async fn execute_with_pool(
    pool: &deadpool_postgres::Pool,
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get a connection from the pool
    let client = pool.get().await
        .map_err(|e| EngineError::DatabaseError(format!("Failed to get connection from pool: {}", e)))?;

    // Get operation type
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("operation".to_string()))?;

    match op_type {
        "query" => execute_query_pooled(&client, operation, bindings, state).await,
        "insert" => execute_insert_pooled(&client, operation, state).await,
        "select" => execute_select_pooled(&client, operation, state).await,
        "update" => execute_update_pooled(&client, operation, state).await,
        "delete" => execute_delete_pooled(&client, operation, state).await,
        _ => Err(EngineError::InvalidOperation(format!("Unknown PostgreSQL operation: {}", op_type))),
    }
}

// ============================================================================
// Pooled versions of operation functions
// These use deadpool's Object<Manager> instead of tokio_postgres::Client
// ============================================================================

/// Execute raw SQL query with pooled connection
///
/// Supports two modes:
/// 1. Bindings mode: Use `{paramName}` in SQL with bindings map (recommended)
/// 2. Legacy params mode: Use `$1, $2...` with params array
async fn execute_query_pooled(
    client: &deadpool_postgres::Object,
    operation: &HashMap<String, Value>,
    bindings: &HashMap<String, String>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let sql_template = get_required_string(operation, "sql", state)?;

    // If bindings are provided, resolve {paramName} placeholders in SQL
    // Otherwise, use legacy params mode
    let rows = if !bindings.is_empty() {
        // Bindings mode: replace {paramName} with actual values
        let sql = resolve_sql_with_bindings(&sql_template, bindings, state);
        tracing::debug!("Executing SQL with bindings: {}", sql);

        client.query(&sql, &[])
            .await
            .map_err(|e| EngineError::DatabaseError(e.to_string()))?
    } else {
        // Legacy params mode: use $1, $2... with params array
        let params = parse_query_params(operation, state)?;
        let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
            .map(|p| p as &(dyn ToSql + Sync))
            .collect();

        client.query(&sql_template, &param_refs)
            .await
            .map_err(|e| EngineError::DatabaseError(e.to_string()))?
    };

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    // Also flatten first row fields to top level for convenience
    if let Some(first_row) = json_rows.first() {
        if let Value::Object(obj) = first_row {
            for (key, value) in obj {
                outputs.insert(key.clone(), value.clone());
            }
        }
    }

    Ok(outputs)
}

/// Execute INSERT operation with pooled connection
async fn execute_insert_pooled(
    client: &deadpool_postgres::Object,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    // Get data to insert
    let data = operation.get("data")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::MissingField("data".to_string()))?;

    // Build column names and placeholders
    let mut columns: Vec<String> = Vec::new();
    let mut placeholders: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();

    for (i, (col, val)) in data.iter().enumerate() {
        columns.push(format!("\"{}\"", col));
        placeholders.push(format!("${}", i + 1));
        let resolved = match val {
            Value::String(s) => resolve_template_vars(s, state),
            other => other.to_string(),
        };
        values.push(resolved);
    }

    // Build RETURNING clause
    let returning = operation.get("returning")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let cols: Vec<String> = arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s))
                .collect();
            format!(" RETURNING {}", cols.join(", "))
        })
        .unwrap_or_default();

    let sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({}){}",
        table,
        columns.join(", "),
        placeholders.join(", "),
        returning
    );

    let params: Vec<&(dyn ToSql + Sync)> = values.iter()
        .map(|v| v as &(dyn ToSql + Sync))
        .collect();

    let rows = client.query(&sql, &params)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().max(1).into()));

    Ok(outputs)
}

/// Execute SELECT operation with pooled connection
async fn execute_select_pooled(
    client: &deadpool_postgres::Object,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    // Get columns (default to *)
    let columns = operation.get("columns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "*".to_string());

    // Build WHERE clause
    let (where_clause, params) = build_where_clause(operation, state)?;

    // Get ORDER BY
    let order_by = get_optional_string(operation, "order_by", state)
        .map(|o| format!(" ORDER BY {}", o))
        .unwrap_or_default();

    // Get LIMIT
    let limit = get_optional_u64(operation, "limit")
        .map(|l| format!(" LIMIT {}", l))
        .unwrap_or_default();

    // Get OFFSET
    let offset = get_optional_u64(operation, "offset")
        .map(|o| format!(" OFFSET {}", o))
        .unwrap_or_default();

    let sql = format!(
        "SELECT {} FROM \"{}\"{}{}{}{}",
        columns, table, where_clause, order_by, limit, offset
    );

    let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect();

    let rows = client.query(&sql, &param_refs)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    Ok(outputs)
}

/// Execute UPDATE operation with pooled connection
async fn execute_update_pooled(
    client: &deadpool_postgres::Object,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    // Get SET data
    let set_data = operation.get("set")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::MissingField("set".to_string()))?;

    // Build SET clause
    let mut set_parts: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    for (col, val) in set_data.iter() {
        let value_str = match val {
            Value::String(s) => {
                let resolved = resolve_template_vars(s, state);
                // Check if it's a SQL function like NOW()
                if resolved.to_uppercase().contains("NOW()") ||
                   resolved.to_uppercase().contains("CURRENT_TIMESTAMP") {
                    set_parts.push(format!("\"{}\" = {}", col, resolved));
                    continue;
                }
                resolved
            }
            other => other.to_string(),
        };
        set_parts.push(format!("\"{}\" = ${}", col, params.len() + 1));
        params.push(value_str);
    }

    // Build WHERE clause
    let (where_clause, where_params) = build_where_clause_with_offset(operation, state, params.len())?;
    params.extend(where_params);

    // Build RETURNING clause
    let returning = operation.get("returning")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let cols: Vec<String> = arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s))
                .collect();
            format!(" RETURNING {}", cols.join(", "))
        })
        .unwrap_or_default();

    let sql = format!(
        "UPDATE \"{}\" SET {}{}{}",
        table,
        set_parts.join(", "),
        where_clause,
        returning
    );

    let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect();

    let rows = client.query(&sql, &param_refs)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    Ok(outputs)
}

/// Execute DELETE operation with pooled connection
async fn execute_delete_pooled(
    client: &deadpool_postgres::Object,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    // Build WHERE clause (required for safety)
    let (where_clause, params) = build_where_clause(operation, state)?;

    if where_clause.is_empty() {
        return Err(EngineError::DatabaseError("DELETE requires WHERE clause for safety".to_string()));
    }

    let sql = format!("DELETE FROM \"{}\"{}",table, where_clause);

    let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect();

    let result = client.execute(&sql, &param_refs)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("count".to_string(), Value::Number(result.into()));

    Ok(outputs)
}
