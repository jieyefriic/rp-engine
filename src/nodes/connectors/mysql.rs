//! MySQL/MariaDB connector implementation
//!
//! Supports operations: query, select, insert, update, delete
//! Mirrors PostgreSQL connector's API but uses mysql_async.

use std::collections::HashMap;
use mysql_async::prelude::*;
use mysql_async::{Pool, Opts, OptsBuilder, Value as MysqlValue, Row};
use serde_json::Value;
use tracing::{debug, warn};

use crate::state::ExecutionState;
use crate::error::EngineError;
use super::{get_required_string, get_optional_string, get_optional_u64, ConnectorContext};

/// Unified MySQL execute function
pub async fn execute(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    ctx: &ConnectorContext<'_>,
) -> Result<HashMap<String, Value>, EngineError> {
    let op_type = operation.get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::MissingField("operation".to_string()))?;

    debug!(operation = %op_type, "Starting MySQL operation");

    let pool = create_pool(ds_config, ctx.state).await?;

    let result = match op_type {
        "query" => execute_query(&pool, operation, ctx.variables, ctx.state).await,
        "select" => execute_select(&pool, operation, ctx.variables, ctx.state).await,
        "insert" => execute_insert(&pool, operation, ctx.variables, ctx.state).await,
        "update" => execute_update(&pool, operation, ctx.variables, ctx.state).await,
        "delete" => execute_delete(&pool, operation, ctx.variables, ctx.state).await,
        _ => Err(EngineError::InvalidOperation(format!(
            "Unknown MySQL operation: {}. Available: query, select, insert, update, delete", op_type
        ))),
    };

    // Drop pool to close connections
    pool.disconnect().await.ok();

    result
}

/// Create a MySQL connection pool
async fn create_pool(
    ds_config: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<Pool, EngineError> {
    let host = get_optional_string(ds_config, "host", state)
        .unwrap_or_else(|| "localhost".to_string());
    let port = get_optional_u64(ds_config, "port").unwrap_or(3306) as u16;
    let username = get_required_string(ds_config, "username", state)?;
    let password = get_required_string(ds_config, "password", state)?;
    let database = get_required_string(ds_config, "database", state)?;

    let opts = OptsBuilder::default()
        .ip_or_hostname(host)
        .tcp_port(port)
        .user(Some(username))
        .pass(Some(password))
        .db_name(Some(database));

    let pool = Pool::new(Opts::from(opts));
    Ok(pool)
}

/// Execute raw SQL query with {paramName} parameterized binding
async fn execute_query(
    pool: &Pool,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let sql_template = get_required_string(operation, "sql", state)?;

    // Replace {paramName} with ? and collect params in order
    let (sql, params) = prepare_parameterized_query(&sql_template, variables);

    debug!(sql = %sql, param_count = params.len(), "Executing MySQL query");

    let mut conn = pool.get_conn().await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL connection failed: {}", e)))?;

    let rows: Vec<Row> = conn.exec(&sql, &params_to_mysql_values(&params))
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL query failed: {}", e)))?;

    let json_rows = rows_to_json(&rows);

    const MAX_ROWS: usize = 10_000;
    let json_rows = if json_rows.len() > MAX_ROWS {
        warn!(actual = json_rows.len(), limit = MAX_ROWS, "MySQL query result truncated");
        json_rows[..MAX_ROWS].to_vec()
    } else {
        json_rows
    };

    debug!(row_count = json_rows.len(), "MySQL query completed");

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    // Flatten first row fields to top level
    if let Some(first_row) = json_rows.first() {
        if let Value::Object(obj) = first_row {
            for (key, value) in obj {
                outputs.insert(key.clone(), value.clone());
            }
        }
    }

    Ok(outputs)
}

/// Execute SELECT with structured config
async fn execute_select(
    pool: &Pool,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;
    let columns = operation.get("columns")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_else(|| "*".to_string());

    let mut sql = format!("SELECT {} FROM `{}`", columns, table);
    let mut params: Vec<Value> = Vec::new();

    // WHERE clause
    if let Some(where_obj) = operation.get("where").and_then(|v| v.as_object()) {
        let mut conditions = Vec::new();
        for (col, val) in where_obj {
            conditions.push(format!("`{}` = ?", col));
            let resolved = resolve_value(val, variables);
            params.push(resolved);
        }
        if !conditions.is_empty() {
            sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
        }
    }

    // ORDER BY
    if let Some(order) = operation.get("order_by").and_then(|v| v.as_str()) {
        sql.push_str(&format!(" ORDER BY {}", order));
    }

    // LIMIT / OFFSET
    if let Some(limit) = get_optional_u64(operation, "limit") {
        sql.push_str(&format!(" LIMIT {}", limit));
    }
    if let Some(offset) = get_optional_u64(operation, "offset") {
        sql.push_str(&format!(" OFFSET {}", offset));
    }

    debug!(sql = %sql, "Executing MySQL select");

    let mut conn = pool.get_conn().await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL connection failed: {}", e)))?;

    let rows: Vec<Row> = conn.exec(&sql, &params_to_mysql_values(&params))
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL select failed: {}", e)))?;

    let json_rows = rows_to_json(&rows);

    let mut outputs = HashMap::new();
    outputs.insert("rows".to_string(), Value::Array(json_rows.clone()));
    outputs.insert("count".to_string(), Value::Number(json_rows.len().into()));

    Ok(outputs)
}

/// Execute INSERT
async fn execute_insert(
    pool: &Pool,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    let data = operation.get("data")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::MissingField("data".to_string()))?;

    let mut columns = Vec::new();
    let mut placeholders = Vec::new();
    let mut params = Vec::new();

    for (col, val) in data {
        columns.push(format!("`{}`", col));
        placeholders.push("?".to_string());
        params.push(resolve_value(val, variables));
    }

    let sql = format!(
        "INSERT INTO `{}` ({}) VALUES ({})",
        table,
        columns.join(", "),
        placeholders.join(", ")
    );

    debug!(sql = %sql, "Executing MySQL insert");

    let mut conn = pool.get_conn().await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL connection failed: {}", e)))?;

    let _result = conn.exec_drop(&sql, &params_to_mysql_values(&params))
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL insert failed: {}", e)))?;

    let last_insert_id = conn.last_insert_id().unwrap_or(0);
    let affected = conn.affected_rows();

    let mut outputs = HashMap::new();
    outputs.insert("count".to_string(), Value::Number(affected.into()));
    outputs.insert("last_insert_id".to_string(), Value::Number(last_insert_id.into()));

    // If RETURNING-like behavior needed, do a follow-up SELECT
    if let Some(returning) = operation.get("returning").and_then(|v| v.as_array()) {
        if !returning.is_empty() && last_insert_id > 0 {
            let cols = returning.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let select_sql = format!("SELECT {} FROM `{}` WHERE id = ?", cols, table);
            let rows: Vec<Row> = conn.exec(&select_sql, (last_insert_id,))
                .await
                .unwrap_or_default();
            let json_rows = rows_to_json(&rows);
            outputs.insert("rows".to_string(), Value::Array(json_rows));
        }
    }

    Ok(outputs)
}

/// Execute UPDATE
async fn execute_update(
    pool: &Pool,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    let set_obj = operation.get("set")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::MissingField("set".to_string()))?;

    let mut set_clauses = Vec::new();
    let mut params = Vec::new();

    for (col, val) in set_obj {
        set_clauses.push(format!("`{}` = ?", col));
        params.push(resolve_value(val, variables));
    }

    let mut sql = format!("UPDATE `{}` SET {}", table, set_clauses.join(", "));

    // WHERE clause (required for safety)
    if let Some(where_obj) = operation.get("where").and_then(|v| v.as_object()) {
        let mut conditions = Vec::new();
        for (col, val) in where_obj {
            conditions.push(format!("`{}` = ?", col));
            params.push(resolve_value(val, variables));
        }
        sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
    } else {
        return Err(EngineError::DatabaseError("UPDATE requires WHERE clause".to_string()));
    }

    debug!(sql = %sql, "Executing MySQL update");

    let mut conn = pool.get_conn().await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL connection failed: {}", e)))?;

    conn.exec_drop(&sql, &params_to_mysql_values(&params))
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL update failed: {}", e)))?;

    let affected = conn.affected_rows();

    let mut outputs = HashMap::new();
    outputs.insert("count".to_string(), Value::Number(affected.into()));

    // RETURNING emulation via SELECT
    if let Some(returning) = operation.get("returning").and_then(|v| v.as_array()) {
        if !returning.is_empty() {
            if let Some(where_obj) = operation.get("where").and_then(|v| v.as_object()) {
                let cols = returning.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut where_conditions = Vec::new();
                let mut where_params = Vec::new();
                for (col, val) in where_obj {
                    where_conditions.push(format!("`{}` = ?", col));
                    where_params.push(resolve_value(val, variables));
                }
                let select_sql = format!("SELECT {} FROM `{}` WHERE {}",
                    cols, table, where_conditions.join(" AND "));
                let rows: Vec<Row> = conn.exec(&select_sql, &params_to_mysql_values(&where_params))
                    .await
                    .unwrap_or_default();
                outputs.insert("rows".to_string(), Value::Array(rows_to_json(&rows)));
            }
        }
    }

    Ok(outputs)
}

/// Execute DELETE
async fn execute_delete(
    pool: &Pool,
    operation: &HashMap<String, Value>,
    variables: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let table = get_required_string(operation, "table", state)?;

    let where_obj = operation.get("where")
        .and_then(|v| v.as_object())
        .ok_or_else(|| EngineError::DatabaseError("DELETE requires WHERE clause".to_string()))?;

    let mut conditions = Vec::new();
    let mut params = Vec::new();

    for (col, val) in where_obj {
        conditions.push(format!("`{}` = ?", col));
        params.push(resolve_value(val, variables));
    }

    let sql = format!("DELETE FROM `{}` WHERE {}", table, conditions.join(" AND "));

    debug!(sql = %sql, "Executing MySQL delete");

    let mut conn = pool.get_conn().await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL connection failed: {}", e)))?;

    conn.exec_drop(&sql, &params_to_mysql_values(&params))
        .await
        .map_err(|e| EngineError::DatabaseError(format!("MySQL delete failed: {}", e)))?;

    let affected = conn.affected_rows();

    let mut outputs = HashMap::new();
    outputs.insert("count".to_string(), Value::Number(affected.into()));

    Ok(outputs)
}

// ============================================================================
// Helpers
// ============================================================================

/// Replace {paramName} in SQL with ? and collect params in order
fn prepare_parameterized_query(
    sql_template: &str,
    variables: &HashMap<String, Value>,
) -> (String, Vec<Value>) {
    let re = regex_lite::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();
    let mut params = Vec::new();

    let sql = re.replace_all(sql_template, |caps: &regex_lite::Captures| {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some(value) = variables.get(name) {
            params.push(value.clone());
        } else {
            params.push(Value::Null);
        }
        "?"
    }).to_string();

    (sql, params)
}

/// Resolve a value, expanding {paramName} references
fn resolve_value(val: &Value, variables: &HashMap<String, Value>) -> Value {
    match val {
        Value::String(s) => {
            if s.starts_with('{') && s.ends_with('}') && !s.contains(' ') {
                let name = &s[1..s.len()-1];
                variables.get(name).cloned().unwrap_or(Value::Null)
            } else {
                val.clone()
            }
        }
        _ => val.clone(),
    }
}

/// Convert JSON values to mysql_async params
fn params_to_mysql_values(values: &[Value]) -> Vec<MysqlValue> {
    values.iter().map(|v| match v {
        Value::Null => MysqlValue::NULL,
        Value::Bool(b) => MysqlValue::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                MysqlValue::from(i)
            } else if let Some(f) = n.as_f64() {
                MysqlValue::from(f)
            } else {
                MysqlValue::NULL
            }
        }
        Value::String(s) => MysqlValue::from(s.as_str()),
        _ => MysqlValue::from(serde_json::to_string(v).unwrap_or_default()),
    }).collect()
}

/// Convert MySQL rows to JSON
fn rows_to_json(rows: &[Row]) -> Vec<Value> {
    rows.iter().map(|row| {
        let mut obj = serde_json::Map::new();
        for col_idx in 0..row.columns().len() {
            let col_name = row.columns()[col_idx].name_str().to_string();
            let value = row_value_to_json(row, col_idx);
            obj.insert(col_name, value);
        }
        Value::Object(obj)
    }).collect()
}

/// Convert a single MySQL column value to JSON
fn row_value_to_json(row: &Row, idx: usize) -> Value {
    // Try various types
    if let Some(val) = row.get_opt::<Option<i64>, _>(idx).and_then(|r| r.ok()) {
        match val {
            Some(i) => Value::Number(i.into()),
            None => Value::Null,
        }
    } else if let Some(val) = row.get_opt::<Option<f64>, _>(idx).and_then(|r| r.ok()) {
        match val {
            Some(f) => serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            None => Value::Null,
        }
    } else if let Some(val) = row.get_opt::<Option<String>, _>(idx).and_then(|r| r.ok()) {
        match val {
            Some(s) => Value::String(s),
            None => Value::Null,
        }
    } else if let Some(val) = row.get_opt::<Option<bool>, _>(idx).and_then(|r| r.ok()) {
        match val {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        }
    } else {
        Value::Null
    }
}
