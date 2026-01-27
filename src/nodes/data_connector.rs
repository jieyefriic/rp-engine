//! Data connector node executor

use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, error};

use crate::schema::YamlNode;
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::NodeExecutor;
use super::connectors;
use super::connectors::ConnectorContext;

/// Executor for data connector nodes
pub struct DataConnectorExecutor;

/// Resolve variables from node config, converting VariableBinding paths to actual Values
fn resolve_variables(
    node: &YamlNode,
    state: &ExecutionState,
) -> HashMap<String, Value> {
    let mut resolved = HashMap::new();

    for (name, binding) in &node.config.variables {
        // For each variable, try to get the first available value from its sources
        let value = binding.sources()
            .iter()
            .filter_map(|path| {
                if path.is_empty() || !path.contains('.') {
                    // Literal value, not a state path
                    None
                } else {
                    // Only return non-null values
                    state.read(path).filter(|v| !v.is_null())
                }
            })
            .next()
            .or_else(|| binding.default_value().cloned())
            .unwrap_or(Value::Null);

        resolved.insert(name.clone(), value);
    }

    resolved
}

#[async_trait]
impl NodeExecutor for DataConnectorExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_config = &node.config;
        let node_id = &node.id;

        // Resolve variables from config (pre-resolve all paths to values)
        let resolved_variables = resolve_variables(node, state);

        // Build unified context with pre-resolved variables
        let ctx = ConnectorContext {
            state,
            config,
            variables: &resolved_variables,
        };

        // V2 mode: use data_source_id reference
        if let Some(data_source_id) = &node_config.data_source_id {
            let data_source = config.data_sources.get(data_source_id)
                .ok_or_else(|| {
                    error!(
                        node = %node_id,
                        data_source_id = %data_source_id,
                        available_data_sources = ?config.data_sources.keys().collect::<Vec<_>>(),
                        "Data source not found"
                    );
                    EngineError::DataSourceNotFound(data_source_id.clone())
                })?;

            let operation_type = node_config.operation.get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            debug!(
                node = %node_id,
                data_source = %data_source_id,
                kind = %data_source.kind,
                operation = %operation_type,
                "Executing data connector"
            );

            let result = match data_source.kind.as_str() {
                "rest_api" => execute_rest_api_v2(&data_source.config, &node_config.operation, state).await,
                "graphql" => execute_graphql_v2(&data_source.config, &node_config.operation, state).await,
                "minio" | "s3" => connectors::minio::execute(&data_source.config, &node_config.operation, &ctx).await,
                "postgresql" | "postgres" => connectors::postgresql::execute(&data_source.config, &node_config.operation, &ctx).await,
                "qdrant" => connectors::qdrant::execute(&data_source.config, &node_config.operation, &ctx).await,
                "redis" => connectors::redis::execute(&data_source.config, &node_config.operation, &ctx).await,
                _ => {
                    error!(
                        node = %node_id,
                        data_source = %data_source_id,
                        kind = %data_source.kind,
                        "Unsupported data source type"
                    );
                    Err(EngineError::UnsupportedSource(data_source.kind.clone()))
                }
            };

            return result.map_err(|e| {
                error!(
                    node = %node_id,
                    data_source = %data_source_id,
                    kind = %data_source.kind,
                    operation = %operation_type,
                    error = %e,
                    "Data connector execution failed"
                );
                e
            });
        }

        // V1 Legacy mode: use source and connection directly
        // Convert connection to ds_config format for unified execution
        let source = node_config.source.as_ref()
            .ok_or(EngineError::MissingSource)?;
        let ds_config = &node_config.connection;

        // For V1 mode, operation info is in connection, so we need to convert it
        // We'll create an operation HashMap that includes the operation type
        let operation = ds_config.clone();

        match source.as_str() {
            "rest_api" => execute_rest_api(node_config, state).await,
            "graphql" => execute_graphql(node_config, state).await,
            "minio" | "s3" => connectors::minio::execute(ds_config, &operation, &ctx).await,
            "postgresql" | "postgres" => connectors::postgresql::execute(ds_config, &operation, &ctx).await,
            "qdrant" => connectors::qdrant::execute(ds_config, &operation, &ctx).await,
            "redis" => connectors::redis::execute(ds_config, &operation, &ctx).await,
            "mysql" => Err(EngineError::UnsupportedSource("mysql not implemented yet".to_string())),
            _ => Err(EngineError::UnsupportedSource(source.clone())),
        }
    }
}

/// Execute a REST API call
async fn execute_rest_api(
    config: &crate::schema::NodeConfig,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let connection = &config.connection;

    // Get URL
    let url_value = connection.get("url")
        .ok_or(EngineError::MissingUrl)?;
    let url_str = url_value.as_str()
        .ok_or_else(|| EngineError::NodeExecutionFailed("URL must be a string".into()))?;

    // Resolve template variables in URL
    let url = resolve_template_vars(url_str, state);

    // Get method
    let method = connection.get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET");

    // Build request
    let client = reqwest::Client::new();
    let mut request = match method.to_uppercase().as_str() {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        "PATCH" => client.patch(&url),
        _ => return Err(EngineError::InvalidMethod(method.to_string())),
    };

    // Add headers
    if let Some(headers) = connection.get("headers") {
        if let Some(headers_obj) = headers.as_object() {
            for (key, value) in headers_obj {
                if let Some(val_str) = value.as_str() {
                    let resolved = resolve_template_vars(val_str, state);
                    request = request.header(key.as_str(), resolved);
                }
            }
        }
    }

    // Add query parameters
    if let Some(params) = connection.get("queryParams") {
        if let Some(params_obj) = params.as_object() {
            let mut query: Vec<(String, String)> = Vec::new();
            for (key, value) in params_obj {
                let val_str = match value {
                    Value::String(s) => resolve_template_vars(s, state),
                    _ => value.to_string(),
                };
                query.push((key.clone(), val_str));
            }
            request = request.query(&query);
        }
    }

    // Add body
    if let Some(body) = connection.get("body") {
        let body_str = match body {
            Value::String(s) => resolve_template_vars(s, state),
            _ => body.to_string(),
        };
        request = request.body(body_str);

        // Set content type if not already set
        if !connection.get("headers")
            .and_then(|h| h.as_object())
            .map(|h| h.contains_key("Content-Type"))
            .unwrap_or(false)
        {
            request = request.header("Content-Type", "application/json");
        }
    }

    // Execute request
    let response = request.send().await?;

    let status = response.status().as_u16();
    let headers: HashMap<String, String> = response.headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str().ok().map(|val| (k.as_str().to_string(), val.to_string()))
        })
        .collect();

    // Parse response body
    let data: Value = response.json().await.unwrap_or(Value::Null);

    // Build outputs
    let mut outputs = HashMap::new();
    outputs.insert("status".to_string(), Value::Number(status.into()));
    outputs.insert("data".to_string(), data);
    outputs.insert("headers".to_string(), serde_json::to_value(headers).unwrap_or(Value::Null));

    Ok(outputs)
}

/// Execute a GraphQL query
async fn execute_graphql(
    config: &crate::schema::NodeConfig,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    let connection = &config.connection;

    // Get URL
    let url_value = connection.get("url")
        .ok_or(EngineError::MissingUrl)?;
    let url = url_value.as_str()
        .ok_or_else(|| EngineError::NodeExecutionFailed("URL must be a string".into()))?;

    // Get query
    let query = connection.get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::NodeExecutionFailed("GraphQL query is required".into()))?;

    // Resolve variables
    let query = resolve_template_vars(query, state);

    // Get GraphQL variables
    let variables = connection.get("variables")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    // Build request body
    let body = serde_json::json!({
        "query": query,
        "variables": variables,
    });

    // Execute request
    let client = reqwest::Client::new();
    let mut request = client.post(url)
        .header("Content-Type", "application/json")
        .json(&body);

    // Add headers
    if let Some(headers) = connection.get("headers") {
        if let Some(headers_obj) = headers.as_object() {
            for (key, value) in headers_obj {
                if let Some(val_str) = value.as_str() {
                    let resolved = resolve_template_vars(val_str, state);
                    request = request.header(key.as_str(), resolved);
                }
            }
        }
    }

    let response = request.send().await?;

    let status = response.status().as_u16();
    let data: Value = response.json().await.unwrap_or(Value::Null);

    // Build outputs
    let mut outputs = HashMap::new();
    outputs.insert("status".to_string(), Value::Number(status.into()));

    // Extract data and errors from GraphQL response
    if let Some(gql_data) = data.get("data") {
        outputs.insert("data".to_string(), gql_data.clone());
    } else {
        outputs.insert("data".to_string(), Value::Null);
    }

    if let Some(errors) = data.get("errors") {
        outputs.insert("errors".to_string(), errors.clone());
    }

    Ok(outputs)
}

/// Execute REST API with V2 config (data source + operation)
async fn execute_rest_api_v2(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get base URL from data source config
    let base_url = ds_config.get("base_url")
        .and_then(|v| v.as_str())
        .ok_or(EngineError::MissingUrl)?;

    // Get path from operation
    let path = operation.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Build full URL
    let url_str = format!("{}{}", base_url.trim_end_matches('/'), path);
    let url = resolve_template_vars(&url_str, state);

    // Get method from operation
    let method = operation.get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET");

    // Build request
    let client = reqwest::Client::new();
    let mut request = match method.to_uppercase().as_str() {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        "PATCH" => client.patch(&url),
        _ => return Err(EngineError::InvalidMethod(method.to_string())),
    };

    // Add headers from data source config
    if let Some(headers) = ds_config.get("headers") {
        if let Some(headers_obj) = headers.as_object() {
            for (key, value) in headers_obj {
                if let Some(val_str) = value.as_str() {
                    let resolved = resolve_template_vars(val_str, state);
                    request = request.header(key.as_str(), resolved);
                }
            }
        }
    }

    // Add auth from data source config
    if let Some(auth_type) = ds_config.get("auth_type").and_then(|v| v.as_str()) {
        if let Some(auth) = ds_config.get("auth") {
            match auth_type {
                "bearer" => {
                    if let Some(token) = auth.get("token").and_then(|v| v.as_str()) {
                        let resolved = resolve_template_vars(token, state);
                        request = request.header("Authorization", format!("Bearer {}", resolved));
                    }
                }
                "api_key" => {
                    if let Some(api_key) = auth.get("api_key").and_then(|v| v.as_str()) {
                        let header_name = auth.get("api_key_header")
                            .and_then(|v| v.as_str())
                            .unwrap_or("X-API-Key");
                        let resolved = resolve_template_vars(api_key, state);
                        request = request.header(header_name, resolved);
                    }
                }
                "basic" => {
                    if let (Some(username), Some(password)) = (
                        auth.get("username").and_then(|v| v.as_str()),
                        auth.get("password").and_then(|v| v.as_str())
                    ) {
                        request = request.basic_auth(username, Some(password));
                    }
                }
                _ => {}
            }
        }
    }

    // Add headers from operation (override data source headers)
    if let Some(headers) = operation.get("headers") {
        if let Some(headers_obj) = headers.as_object() {
            for (key, value) in headers_obj {
                if let Some(val_str) = value.as_str() {
                    let resolved = resolve_template_vars(val_str, state);
                    request = request.header(key.as_str(), resolved);
                }
            }
        }
    }

    // Add query parameters from operation
    if let Some(params) = operation.get("query_params") {
        if let Some(params_obj) = params.as_object() {
            let mut query: Vec<(String, String)> = Vec::new();
            for (key, value) in params_obj {
                let val_str = match value {
                    Value::String(s) => resolve_template_vars(s, state),
                    _ => value.to_string(),
                };
                query.push((key.clone(), val_str));
            }
            request = request.query(&query);
        }
    }

    // Add body from operation
    if let Some(body) = operation.get("body") {
        let body_str = match body {
            Value::String(s) => resolve_template_vars(s, state),
            _ => body.to_string(),
        };
        request = request.body(body_str);

        // Set content type based on body_type
        let body_type = operation.get("body_type")
            .and_then(|v| v.as_str())
            .unwrap_or("json");
        match body_type {
            "json" => {
                request = request.header("Content-Type", "application/json");
            }
            "form" => {
                request = request.header("Content-Type", "application/x-www-form-urlencoded");
            }
            _ => {}
        }
    }

    // Execute request
    let response = request.send().await?;

    let status = response.status().as_u16();
    let headers: HashMap<String, String> = response.headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str().ok().map(|val| (k.as_str().to_string(), val.to_string()))
        })
        .collect();

    // Parse response body
    let data: Value = response.json().await.unwrap_or(Value::Null);

    // Build outputs
    let mut outputs = HashMap::new();
    outputs.insert("status".to_string(), Value::Number(status.into()));
    outputs.insert("data".to_string(), data);
    outputs.insert("headers".to_string(), serde_json::to_value(headers).unwrap_or(Value::Null));

    Ok(outputs)
}

/// Execute GraphQL with V2 config (data source + operation)
async fn execute_graphql_v2(
    ds_config: &HashMap<String, Value>,
    operation: &HashMap<String, Value>,
    state: &ExecutionState,
) -> Result<HashMap<String, Value>, EngineError> {
    // Get endpoint from data source config
    let endpoint = ds_config.get("endpoint")
        .and_then(|v| v.as_str())
        .ok_or(EngineError::MissingUrl)?;

    // Get query from operation
    let query = operation.get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::NodeExecutionFailed("GraphQL query is required".into()))?;

    // Resolve variables in query
    let query = resolve_template_vars(query, state);

    // Get GraphQL variables from operation
    let variables = operation.get("variables")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    // Build request body
    let body = serde_json::json!({
        "query": query,
        "variables": variables,
    });

    // Execute request
    let client = reqwest::Client::new();
    let mut request = client.post(endpoint)
        .header("Content-Type", "application/json")
        .json(&body);

    // Add headers from data source config
    if let Some(headers) = ds_config.get("headers") {
        if let Some(headers_obj) = headers.as_object() {
            for (key, value) in headers_obj {
                if let Some(val_str) = value.as_str() {
                    let resolved = resolve_template_vars(val_str, state);
                    request = request.header(key.as_str(), resolved);
                }
            }
        }
    }

    // Add auth from data source config
    if let Some(auth_type) = ds_config.get("auth_type").and_then(|v| v.as_str()) {
        if let Some(auth) = ds_config.get("auth") {
            match auth_type {
                "bearer" => {
                    if let Some(token) = auth.get("token").and_then(|v| v.as_str()) {
                        let resolved = resolve_template_vars(token, state);
                        request = request.header("Authorization", format!("Bearer {}", resolved));
                    }
                }
                "api_key" => {
                    if let Some(api_key) = auth.get("api_key").and_then(|v| v.as_str()) {
                        let header_name = auth.get("api_key_header")
                            .and_then(|v| v.as_str())
                            .unwrap_or("X-API-Key");
                        let resolved = resolve_template_vars(api_key, state);
                        request = request.header(header_name, resolved);
                    }
                }
                _ => {}
            }
        }
    }

    let response = request.send().await?;

    let status = response.status().as_u16();
    let data: Value = response.json().await.unwrap_or(Value::Null);

    // Build outputs
    let mut outputs = HashMap::new();
    outputs.insert("status".to_string(), Value::Number(status.into()));

    // Extract data and errors from GraphQL response
    if let Some(gql_data) = data.get("data") {
        outputs.insert("data".to_string(), gql_data.clone());
    } else {
        outputs.insert("data".to_string(), Value::Null);
    }

    if let Some(errors) = data.get("errors") {
        outputs.insert("errors".to_string(), errors.clone());
    }

    Ok(outputs)
}

/// Resolve {{variable}} templates in a string
fn resolve_template_vars(template: &str, state: &ExecutionState) -> String {
    let re = regex_lite::Regex::new(r"\{\{([^}]+)\}\}").unwrap();

    re.replace_all(template, |caps: &regex_lite::Captures| {
        let path = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        state.read(path)
            .map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            })
            .unwrap_or_else(|| format!("{{{{{}}}}}", path))
    }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_template_vars() {
        let mut state = ExecutionState::new(HashMap::new());

        let mut outputs = HashMap::new();
        outputs.insert("api_url".to_string(), Value::String("https://api.example.com".to_string()));
        state.write("think", outputs);

        let result = resolve_template_vars("GET {{think.api_url}}/users", &state);
        assert_eq!(result, "GET https://api.example.com/users");
    }
}
