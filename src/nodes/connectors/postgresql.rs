//! PostgreSQL data connector implementation
//!
//! Supports operations: query, insert, select, update, delete
//!
//! Recommended: `variables` map with `{paramName}` syntax in SQL (parameterized, type-safe)
//! Deprecated: `params` array with `{{node.field}}` syntax (string interpolation, unsafe)

use std::collections::HashMap;
use async_trait::async_trait;
use tokio_postgres::{NoTls, Row};
use tokio_postgres::types::{ToSql, Type as PgType, IsNull, Json};
use bytes::BytesMut;
use serde_json::Value;
use tracing::{debug, error};
use chrono::{DateTime, Utc, NaiveDateTime};

use deadpool_postgres::GenericClient;

use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, resolve_template_vars, ConnectorContext};

// ============================================================================
// PgClient trait — unifies tokio_postgres::Client and deadpool_postgres::Object
// ============================================================================

/// Internal trait abstracting over direct and pooled PostgreSQL connections.
///
/// Both `tokio_postgres::Client` and `deadpool_postgres::Object` expose the same
/// `.query()` / `.execute()` API. This trait eliminates the need for duplicate
/// implementations of every operation.
#[async_trait]
trait PgClient: Send + Sync {
    async fn query(
        &self,
        statement: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, tokio_postgres::Error>;

    async fn pg_execute(
        &self,
        statement: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<u64, tokio_postgres::Error>;
}

#[async_trait]
impl PgClient for tokio_postgres::Client {
    async fn query(
        &self,
        statement: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, tokio_postgres::Error> {
        tokio_postgres::Client::query(self, statement, params).await
    }

    async fn pg_execute(
        &self,
        statement: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<u64, tokio_postgres::Error> {
        tokio_postgres::Client::execute(self, statement, params).await
    }
}

#[async_trait]
impl PgClient for deadpool_postgres::Object {
    async fn query(
        &self,
        statement: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, tokio_postgres::Error> {
        GenericClient::query(self, statement, params).await
    }

    async fn pg_execute(
        &self,
        statement: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<u64, tokio_postgres::Error> {
        GenericClient::execute(self, statement, params).await
    }
}

// ============================================================================
// SqlParam — type-safe SQL parameter wrapper
// ============================================================================

/// Wrapper enum for dynamic SQL parameter types.
/// Used by all operations (query, insert, select, update, delete) to preserve
/// type information when binding values to PostgreSQL queries.
#[derive(Debug)]
enum SqlParam {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Json(Json<Value>),
    Uuid(uuid::Uuid),
    StringArray(Vec<String>),
    Null,
}

impl ToSql for SqlParam {
    fn to_sql(&self, ty: &PgType, out: &mut BytesMut) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self {
            SqlParam::String(s) => s.to_sql(ty, out),
            SqlParam::Int(i) => {
                match *ty {
                    PgType::INT2 => (*i as i16).to_sql(ty, out),
                    PgType::INT4 => (*i as i32).to_sql(ty, out),
                    PgType::FLOAT4 => (*i as f32).to_sql(ty, out),
                    PgType::FLOAT8 => (*i as f64).to_sql(ty, out),
                    _ => i.to_sql(ty, out),
                }
            }
            SqlParam::Float(f) => {
                match *ty {
                    PgType::FLOAT4 => (*f as f32).to_sql(ty, out),
                    PgType::INT4 => (*f as i32).to_sql(ty, out),
                    PgType::INT8 => (*f as i64).to_sql(ty, out),
                    _ => f.to_sql(ty, out),
                }
            }
            SqlParam::Bool(b) => b.to_sql(ty, out),
            SqlParam::Json(j) => j.to_sql(ty, out),
            SqlParam::Uuid(u) => u.to_sql(ty, out),
            SqlParam::StringArray(arr) => {
                // Check if target is a JSONB/JSON column - convert to JSON array
                match *ty {
                    PgType::JSONB | PgType::JSON => {
                        let json_arr = Value::Array(arr.iter().map(|s| Value::String(s.clone())).collect());
                        Json(json_arr).to_sql(ty, out)
                    }
                    _ => arr.to_sql(ty, out),
                }
            }
            SqlParam::Null => Ok(IsNull::Yes),
        }
    }

    fn accepts(ty: &PgType) -> bool {
        <String as ToSql>::accepts(ty)
            || <i64 as ToSql>::accepts(ty)
            || <i32 as ToSql>::accepts(ty)
            || <i16 as ToSql>::accepts(ty)
            || <f64 as ToSql>::accepts(ty)
            || <f32 as ToSql>::accepts(ty)
            || <bool as ToSql>::accepts(ty)
            || <Json<Value> as ToSql>::accepts(ty)
            || <uuid::Uuid as ToSql>::accepts(ty)
            || <Vec<String> as ToSql>::accepts(ty)
    }

    tokio_postgres::types::to_sql_checked!();
}

// ============================================================================
// Parameterized query helpers
// ============================================================================

/// Prepare a parameterized SQL query from a template
///
/// Replaces {paramName} patterns with $1, $2, ... placeholders
/// and returns the SQL string along with the parameter values.
///
/// For example, with:
/// - sql: "SELECT * FROM users WHERE id = {userId} AND name = {name}"
/// - variables: {"userId": Value::String("123"), "name": Value::String("test")}
///
/// Returns: ("SELECT * FROM users WHERE id = $1 AND name = $2", [SqlParam::String("123"), SqlParam::String("test")])
fn prepare_parameterized_query(
    sql: &str,
    variables: &HashMap<String, Value>,
) -> (String, Vec<SqlParam>) {
    let re = regex_lite::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();
    let mut params: Vec<SqlParam> = Vec::new();
    let mut param_index = 0;

    let result_sql = re.replace_all(sql, |caps: &regex_lite::Captures| {
        let param_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");

        if let Some(value) = variables.get(param_name) {
            param_index += 1;
            let param = value_to_sql_param(value);
            params.push(param);
            format!("${}", param_index)
        } else {
            // No variable found, leave the placeholder as-is
            format!("{{{}}}", param_name)
        }
    }).to_string();

    (result_sql, params)
}

/// Convert a serde_json::Value to SqlParam
fn value_to_sql_param(value: &Value) -> SqlParam {
    match value {
        Value::String(s) => {
            // Try to parse as UUID first (for UUID columns)
            if let Ok(uuid) = uuid::Uuid::parse_str(s) {
                SqlParam::Uuid(uuid)
            } else {
                SqlParam::String(s.clone())
            }
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlParam::Int(i)
            } else if let Some(f) = n.as_f64() {
                SqlParam::Float(f)
            } else {
                SqlParam::String(n.to_string())
            }
        }
        Value::Bool(b) => SqlParam::Bool(*b),
        Value::Null => SqlParam::Null,
        Value::Array(arr) => {
            // If all elements are strings, use StringArray for text[]/varchar[] columns
            // Otherwise fall back to JSON for jsonb columns
            let all_strings = arr.iter().all(|v| v.is_string());
            if all_strings {
                let strings: Vec<String> = arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                SqlParam::StringArray(strings)
            } else {
                SqlParam::Json(Json(value.clone()))
            }
        }
        Value::Object(_) => SqlParam::Json(Json(value.clone())),
    }
}

// ============================================================================
// Main execute entry point
// ============================================================================

/// Unified PostgreSQL execute function
///
/// Takes data source config, operation config, and connector context.
/// Uses connection pool from context if available for better performance.
pub async fn execute(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    ctx: &ConnectorContext<'_>,
) -> Result<HashMap<String, Value>, EngineError> {
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!(
                operation_config = ?operation,
                "PostgreSQL operation type not specified"
            );
            EngineError::MissingField("operation".to_string())
        })?;

    debug!(operation = %op_type, "Starting PostgreSQL operation");

    // Acquire client: prefer pool, fallback to ad-hoc connection
    let client: Box<dyn PgClient> = if let Some(pool) = find_pool_for_config(ds_config, ctx) {
        debug!(operation = %op_type, "Using pooled connection");
        let obj = pool.get().await
            .map_err(|e| {
                let pool_status = pool.status();
                error!(
                    error = %e,
                    pool_size = pool_status.size,
                    pool_available = pool_status.available,
                    pool_waiting = pool_status.waiting,
                    "Failed to get connection from PostgreSQL pool"
                );
                EngineError::DatabaseError(format!(
                    "Failed to get connection from pool (size: {}, available: {}, waiting: {}): {}",
                    pool_status.size, pool_status.available, pool_status.waiting, e
                ))
            })?;
        Box::new(obj)
    } else {
        debug!(operation = %op_type, "Creating new connection (no pool available)");
        let conn_string = get_connection_string(ds_config, ctx.state)?;
        let (client, db_connection) = tokio_postgres::connect(&conn_string, NoTls)
            .await
            .map_err(|e| {
                error!(
                    error = %e,
                    host = ds_config.get("host").and_then(|v| v.as_str()).unwrap_or("unknown"),
                    database = ds_config.get("database").and_then(|v| v.as_str()).unwrap_or("unknown"),
                    "Failed to connect to PostgreSQL"
                );
                EngineError::DatabaseError(format!("Connection failed to PostgreSQL: {}", e))
            })?;

        tokio::spawn(async move {
            if let Err(e) = db_connection.await {
                error!(error = %e, "PostgreSQL connection error during execution");
            }
        });

        Box::new(client)
    };

    // Dispatch to operation handler
    match op_type {
        "query" => execute_query(&*client, operation, ctx.variables, ctx.state).await,
        "insert" => execute_insert(&*client, operation, ctx.variables, ctx.state).await,
        "select" => execute_select(&*client, operation, ctx.variables, ctx.state).await,
        "update" => execute_update(&*client, operation, ctx.variables, ctx.state).await,
        "delete" => execute_delete(&*client, operation, ctx.variables, ctx.state).await,
        _ => {
            error!(operation = %op_type, "Unknown PostgreSQL operation type");
            Err(EngineError::InvalidOperation(format!("Unknown PostgreSQL operation: {}", op_type)))
        }
    }
}

/// Try to find a connection pool for the given data source config
fn find_pool_for_config<'a>(
    ds_config: &HashMap<String, Value>,
    ctx: &'a ConnectorContext<'_>,
) -> Option<&'a deadpool_postgres::Pool> {
    for (ds_id, ds_def) in &ctx.config.data_sources {
        if ds_def.kind == "postgresql" || ds_def.kind == "postgres" {
            let ds_url = get_optional_string(&ds_def.config, "url", ctx.state);
            let cfg_url = get_optional_string(ds_config, "url", ctx.state);
            if ds_url == cfg_url && ds_url.is_some() {
                return ctx.config.pools.get_pg_pool(ds_id);
            }
        }
    }
    None
}

/// Build PostgreSQL connection string
fn get_connection_string(
    connection: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<String, EngineError> {
    if let Some(url) = get_optional_string(connection, "url", state) {
        return Ok(url);
    }

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

// ============================================================================
// Operation implementations (single version, works with any PgClient)
// ============================================================================

/// Execute raw SQL query
///
/// Supports two modes:
/// 1. Variables mode: Use `{paramName}` in SQL with pre-resolved variables map (recommended)
/// 2. Legacy params mode: Use `$1, $2...` with params array (deprecated)
async fn execute_query(
    client: &dyn PgClient,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let sql_template = get_required_string(operation, "sql", state)?;

    let rows = if !variables.is_empty() {
        let (sql, params) = prepare_parameterized_query(&sql_template, variables);
        let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
            .map(|p| p as &(dyn ToSql + Sync))
            .collect();

        debug!(sql = %sql, params = ?params, "Executing parameterized SQL query");
        client.query(&sql, &param_refs)
            .await
            .map_err(|e| {
                error!(error = %e, sql = %sql, params = ?params, "PostgreSQL query failed");
                EngineError::DatabaseError(format!(
                    "Query failed: {} | SQL: {} | Params: {:?}", e, sql, params
                ))
            })?
    } else {
        // Legacy params mode (DEPRECATED)
        tracing::warn!("DEPRECATED: PostgreSQL query using legacy params mode. Migrate to 'variables' + '{{paramName}}' parameterized binding.");
        let params = parse_query_params(operation, state)?;
        let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
            .map(|p| p as &(dyn ToSql + Sync))
            .collect();

        debug!(sql = %sql_template, params = ?params, "Executing SQL with params (legacy)");
        client.query(&sql_template, &param_refs)
            .await
            .map_err(|e| {
                error!(error = %e, sql = %sql_template, params = ?params, "PostgreSQL query failed");
                EngineError::DatabaseError(format!(
                    "Query failed: {} | SQL: {} | Params: {:?}", e, sql_template, params
                ))
            })?
    };

    let json_rows = rows_to_json(&rows);

    // Safety limit: prevent OOM from huge result sets
    const MAX_ROWS: usize = 10_000;
    if json_rows.len() > MAX_ROWS {
        tracing::warn!(
            actual = json_rows.len(),
            limit = MAX_ROWS,
            "PostgreSQL query returned too many rows, truncating"
        );
    }
    let json_rows = if json_rows.len() > MAX_ROWS {
        json_rows[..MAX_ROWS].to_vec()
    } else {
        json_rows
    };

    debug!(row_count = json_rows.len(), "PostgreSQL query completed successfully");

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    // Flatten first row fields to top level for convenience
    if let Some(first_row) = json_rows.first() {
        if let Value::Object(obj) = first_row {
            for (key, value) in obj {
                outputs.insert(key.clone(), value.clone());
            }
        }
    }

    Ok(outputs)
}

/// Execute INSERT operation
///
/// Uses SqlParam for type-safe parameter binding.
/// Values from `data` are converted via `value_to_sql_param()` preserving
/// their original types (Int, Bool, Json, Uuid, etc.)
async fn execute_insert(
    client: &dyn PgClient,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    let data = operation.get("data")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::MissingField("data".to_string()))?;

    let mut columns: Vec<String> = Vec::new();
    let mut placeholders: Vec<String> = Vec::new();
    let mut params: Vec<SqlParam> = Vec::new();

    for (i, (col, val)) in data.iter().enumerate() {
        columns.push(format!("\"{}\"", col));
        placeholders.push(format!("${}", i + 1));
        let resolved = resolve_value(val, variables);
        params.push(value_to_sql_param(&resolved));
    }

    let returning = build_returning_clause(operation);

    let sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({}){}",
        table,
        columns.join(", "),
        placeholders.join(", "),
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
    outputs.insert("count".to_string(), Value::Number(json_rows.len().max(1).into()));

    Ok(outputs)
}

/// Execute SELECT operation
async fn execute_select(
    client: &dyn PgClient,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

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

    let (where_clause, params) = build_where_clause(operation, variables);

    let order_by = get_optional_string(operation, "order_by", state)
        .map(|o| format!(" ORDER BY {}", o))
        .unwrap_or_default();

    let limit = get_optional_u64(operation, "limit")
        .map(|l| format!(" LIMIT {}", l))
        .unwrap_or_default();

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

/// Execute UPDATE operation
///
/// Uses SqlParam for type-safe SET values.
async fn execute_update(
    client: &dyn PgClient,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    let set_data = operation.get("set")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::MissingField("set".to_string()))?;

    let mut set_parts: Vec<String> = Vec::new();
    let mut params: Vec<SqlParam> = Vec::new();

    for (col, val) in set_data.iter() {
        let resolved = resolve_value(val, variables);
        params.push(value_to_sql_param(&resolved));
        set_parts.push(format!("\"{}\" = ${}", col, params.len()));
    }

    let (where_clause, where_params) = build_where_clause_with_offset(operation, variables, params.len());
    params.extend(where_params);

    let returning = build_returning_clause(operation);

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

/// Execute DELETE operation
async fn execute_delete(
    client: &dyn PgClient,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    let (where_clause, params) = build_where_clause(operation, variables);

    if where_clause.is_empty() {
        return Err(EngineError::DatabaseError("DELETE requires WHERE clause for safety".to_string()));
    }

    let sql = format!("DELETE FROM \"{}\"{}",table, where_clause);

    let param_refs: Vec<&(dyn ToSql + Sync)> = params.iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect();

    let result = client.pg_execute(&sql, &param_refs)
        .await
        .map_err(|e| EngineError::DatabaseError(e.to_string()))?;

    let mut outputs = HashMap::new();
    outputs.insert("count".to_string(), Value::Number(result.into()));

    Ok(outputs)
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Resolve a JSON value through variables.
///
/// If the value is a string containing `{paramName}` patterns, resolve them
/// from the pre-resolved variables map. For non-string values, return as-is.
fn resolve_value(val: &Value, variables: &HashMap<String, Value>) -> Value {
    match val {
        Value::String(s) => {
            let trimmed = s.trim();
            // Exact single-variable reference: "{varName}" → inject raw typed value
            if trimmed.starts_with('{') && trimmed.ends_with('}') && !trimmed.contains(' ') {
                let inner = &trimmed[1..trimmed.len() - 1];
                if !inner.is_empty() && !inner.contains('{') && !inner.contains('}') {
                    if let Some(resolved) = variables.get(inner) {
                        return resolved.clone();
                    }
                }
            }
            // Mixed template or no match: do string interpolation
            let re = regex_lite::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();
            let result = re.replace_all(s, |caps: &regex_lite::Captures| {
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
            }).to_string();
            Value::String(result)
        }
        other => other.clone(),
    }
}

/// Parse query parameters from config (legacy $1, $2... mode)
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

/// Build WHERE clause from operation config using SqlParam for type safety.
fn build_where_clause(
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
) -> (String, Vec<SqlParam>) {
    build_where_clause_with_offset(operation, variables, 0)
}

/// Build WHERE clause with parameter offset, using SqlParam for type safety.
fn build_where_clause_with_offset(
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    offset: usize,
) -> (String, Vec<SqlParam>) {
    let where_obj = match operation.get("where").and_then(|v| v.as_object()) {
        Some(obj) => obj,
        None => return (String::new(), Vec::new()),
    };

    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<SqlParam> = Vec::new();

    for (col, val) in where_obj {
        let resolved = resolve_value(val, variables);
        params.push(value_to_sql_param(&resolved));
        conditions.push(format!("\"{}\" = ${}", col, offset + params.len()));
    }

    if conditions.is_empty() {
        (String::new(), Vec::new())
    } else {
        (format!(" WHERE {}", conditions.join(" AND ")), params)
    }
}

/// Build RETURNING clause from operation config.
fn build_returning_clause(operation: &HashMap<String, Value>) -> String {
    operation.get("returning")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let cols: Vec<String> = arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s))
                .collect();
            format!(" RETURNING {}", cols.join(", "))
        })
        .unwrap_or_default()
}

// ============================================================================
// Row conversion
// ============================================================================

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
        Type::NUMERIC => {
            // NUMERIC/DECIMAL → read raw bytes and convert to string, then parse
            // tokio-postgres doesn't support NUMERIC→f64 natively without rust_decimal
            // Fallback: read the raw text representation via the text output format
            match row.try_get::<_, Option<f64>>(idx) {
                Ok(Some(f)) => serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
                Ok(None) => Value::Null,
                Err(_) => {
                    // If f64 fails, try string representation
                    row.try_get::<_, Option<String>>(idx)
                        .ok()
                        .flatten()
                        .and_then(|s| s.parse::<f64>().ok())
                        .and_then(|f| serde_json::Number::from_f64(f))
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                }
            }
        }
        Type::JSON | Type::JSONB => row.get::<_, Option<Value>>(idx)
            .unwrap_or(Value::Null),
        Type::UUID => row.get::<_, Option<uuid::Uuid>>(idx)
            .map(|v| Value::String(v.to_string()))
            .unwrap_or(Value::Null),
        Type::TIMESTAMP => row.get::<_, Option<NaiveDateTime>>(idx)
            .map(|v| Value::String(v.format("%Y-%m-%d %H:%M:%S").to_string()))
            .unwrap_or(Value::Null),
        Type::TIMESTAMPTZ => row.get::<_, Option<DateTime<Utc>>>(idx)
            .map(|v| Value::String(v.to_rfc3339()))
            .unwrap_or(Value::Null),
        _ => row.get::<_, Option<String>>(idx)
            .map(Value::String)
            .unwrap_or(Value::Null),
    }
}
