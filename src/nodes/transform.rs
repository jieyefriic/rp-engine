//! Transform node executor - Rhai scripting for pure computation

use std::collections::HashMap;
use async_trait::async_trait;
use rhai::{Engine as RhaiEngine, Scope, Dynamic, Map as RhaiMap};
use serde_json::Value;
use tracing::{debug, info, error};

use crate::schema::YamlNode;
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::NodeExecutor;

/// Executor for transform nodes — runs Rhai scripts for pure computation
pub struct TransformExecutor;

/// Convert serde_json::Value to Rhai Dynamic
fn value_to_dynamic(value: &Value) -> Dynamic {
    match value {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => Dynamic::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                Dynamic::from(f)
            } else {
                Dynamic::UNIT
            }
        }
        Value::String(s) => Dynamic::from(s.clone()),
        Value::Array(arr) => {
            let rhai_arr: Vec<Dynamic> = arr.iter().map(value_to_dynamic).collect();
            Dynamic::from(rhai_arr)
        }
        Value::Object(map) => {
            let mut rhai_map = RhaiMap::new();
            for (k, v) in map {
                rhai_map.insert(k.clone().into(), value_to_dynamic(v));
            }
            Dynamic::from(rhai_map)
        }
    }
}

/// Convert Rhai Dynamic back to serde_json::Value
fn dynamic_to_value(val: Dynamic) -> Value {
    if val.is_unit() {
        Value::Null
    } else if val.is_bool() {
        Value::Bool(val.as_bool().unwrap())
    } else if val.is_int() {
        Value::Number(serde_json::Number::from(val.as_int().unwrap()))
    } else if val.is_float() {
        if let Some(n) = serde_json::Number::from_f64(val.as_float().unwrap()) {
            Value::Number(n)
        } else {
            Value::Null
        }
    } else if val.is_string() {
        Value::String(val.into_string().unwrap())
    } else if val.is_array() {
        let arr = val.into_array().unwrap();
        Value::Array(arr.into_iter().map(dynamic_to_value).collect())
    } else if val.is_map() {
        let map = val.cast::<RhaiMap>();
        let mut json_map = serde_json::Map::new();
        for (k, v) in map {
            json_map.insert(k.to_string(), dynamic_to_value(v));
        }
        Value::Object(json_map)
    } else {
        // Fallback: convert via debug representation
        Value::String(format!("{:?}", val))
    }
}

/// Create a Rhai engine with built-in functions registered
fn create_rhai_engine() -> RhaiEngine {
    let mut engine = RhaiEngine::new();

    // Limit script execution to prevent abuse
    engine.set_max_operations(100_000);
    engine.set_max_string_size(1_048_576); // 1MB
    engine.set_max_array_size(10_000);
    engine.set_max_map_size(10_000);

    // now() → ISO8601 timestamp string (UTC)
    engine.register_fn("now", || -> String {
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
    });

    // uuid() → random UUID v4 string
    engine.register_fn("uuid", || -> String {
        uuid::Uuid::new_v4().to_string()
    });

    // json_encode(value) → JSON string
    engine.register_fn("json_encode", |val: Dynamic| -> String {
        let json_val = dynamic_to_value(val);
        serde_json::to_string(&json_val).unwrap_or_default()
    });

    // unique(array) → deduplicated array (preserves order)
    engine.register_fn("unique", |arr: rhai::Array| -> rhai::Array {
        let mut seen = Vec::new();
        let mut result = Vec::new();
        for item in arr {
            let key = format!("{:?}", item);
            if !seen.contains(&key) {
                seen.push(key);
                result.push(item);
            }
        }
        result
    });

    engine
}

#[async_trait]
impl NodeExecutor for TransformExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_config = &node.config;
        let node_id = &node.id;

        debug!(node = %node_id, "Starting transform node execution");

        // Resolve script source: inline `script` takes precedence, then `file` lookup
        let (script_source, script_label) = if let Some(inline) = &node_config.script {
            // Inline script embedded directly in node config
            (inline.as_str(), "<inline>")
        } else if let Some(script_name) = &node_config.file {
            // Look up by name from workflow.transforms or Engine-level transforms
            let source = config.transforms.get(script_name.as_str())
                .ok_or_else(|| {
                    error!(
                        node = %node_id,
                        script = %script_name,
                        available = ?config.transforms.keys().collect::<Vec<_>>(),
                        "Transform script not found"
                    );
                    EngineError::TransformError(
                        format!("Node '{}': Transform script '{}' not found. Available: {:?}",
                            node_id, script_name, config.transforms.keys().collect::<Vec<_>>())
                    )
                })?;
            (source.as_str(), script_name.as_str())
        } else {
            return Err(EngineError::TransformError(
                format!("Node '{}': Transform node requires 'script' or 'file' in config", node_id)
            ));
        };

        // Resolve variables from state (shared logic handles literals + state paths)
        let variables = super::resolve_variables(&node_config.variables, state);

        debug!(
            node = %node_id,
            script = %script_label,
            variable_count = variables.len(),
            "Executing transform script"
        );

        // Create Rhai engine and scope
        let engine = create_rhai_engine();
        let mut scope = Scope::new();

        // Inject resolved variables into Rhai scope
        for (name, value) in &variables {
            scope.push_dynamic(name.clone(), value_to_dynamic(value));
        }

        // Inject _vars: aggregated map of all resolved variables
        let mut vars_map = RhaiMap::new();
        for (name, value) in &variables {
            vars_map.insert(name.clone().into(), value_to_dynamic(value));
        }
        scope.push_dynamic("_vars".to_string(), Dynamic::from(vars_map));

        // Execute script
        let result = engine.eval_with_scope::<Dynamic>(&mut scope, script_source)
            .map_err(|e| {
                error!(
                    node = %node_id,
                    script = %script_label,
                    error = %e,
                    "Transform script execution failed"
                );
                EngineError::TransformError(
                    format!("Node '{}': Script '{}' execution failed: {}", node_id, script_label, e)
                )
            })?;

        // Convert result to output HashMap
        let output = if result.is_map() {
            // Map → spread keys as output fields
            let map = result.cast::<RhaiMap>();
            let mut out = HashMap::new();
            for (k, v) in map {
                out.insert(k.to_string(), dynamic_to_value(v));
            }
            out
        } else {
            // Non-map → wrap as {"result": value}
            let mut out = HashMap::new();
            out.insert("result".to_string(), dynamic_to_value(result));
            out
        };

        info!(
            node = %node_id,
            script = %script_label,
            output_keys = ?output.keys().collect::<Vec<_>>(),
            "Transform script completed"
        );

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{NodeType, NodeConfig, VariableBinding};

    fn make_config_with_transforms(transforms: HashMap<String, String>) -> ExecutionConfig {
        let mut config = ExecutionConfig::default();
        config.transforms = transforms;
        config
    }

    fn make_transform_node(id: &str, file: &str, variables: HashMap<String, VariableBinding>) -> YamlNode {
        YamlNode {
            id: id.to_string(),
            node_type: NodeType::Transform,
            config: NodeConfig {
                file: Some(file.to_string()),
                variables,
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn test_transform_basic_map_return() {
        let mut transforms = HashMap::new();
        transforms.insert("test".to_string(), r#"
            #{
                "greeting": "hello world",
                "count": 42
            }
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "test", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        assert_eq!(result.get("greeting").unwrap(), &Value::String("hello world".into()));
        assert_eq!(result.get("count").unwrap(), &Value::Number(42.into()));
    }

    #[tokio::test]
    async fn test_transform_non_map_return() {
        let mut transforms = HashMap::new();
        transforms.insert("calc".to_string(), "40 + 2".to_string());

        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "calc", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        assert_eq!(result.get("result").unwrap(), &Value::Number(42.into()));
    }

    #[tokio::test]
    async fn test_transform_variable_injection() {
        let mut transforms = HashMap::new();
        transforms.insert("greet".to_string(), r#"
            #{
                "message": "Hello, " + name
            }
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let mut state = ExecutionState::new(HashMap::new());
        state.write("start", {
            let mut m = HashMap::new();
            m.insert("user_name".to_string(), Value::String("Alice".to_string()));
            m
        });

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), VariableBinding::Single("start.user_name".to_string()));
        let node = make_transform_node("t1", "greet", vars);

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        assert_eq!(result.get("message").unwrap(), &Value::String("Hello, Alice".into()));
    }

    #[tokio::test]
    async fn test_transform_builtin_now() {
        let mut transforms = HashMap::new();
        transforms.insert("time".to_string(), r#"
            let t = now();
            #{
                "time": t
            }
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "time", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        let time_str = result.get("time").unwrap().as_str().unwrap();
        // Should be a valid datetime string like "2026-02-11 15:30:00"
        assert!(time_str.len() >= 19, "time string too short: {}", time_str);
    }

    #[tokio::test]
    async fn test_transform_builtin_uuid() {
        let mut transforms = HashMap::new();
        transforms.insert("gen_id".to_string(), r#"
            #{
                "id": uuid()
            }
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "gen_id", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        let id = result.get("id").unwrap().as_str().unwrap();
        // UUID v4 format: 8-4-4-4-12
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
    }

    #[tokio::test]
    async fn test_transform_builtin_json_encode() {
        let mut transforms = HashMap::new();
        transforms.insert("encode".to_string(), r#"
            let data = #{ "key": "value", "num": 42 };
            #{
                "json": json_encode(data)
            }
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "encode", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        let json_str = result.get("json").unwrap().as_str().unwrap();
        let parsed: Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed["key"], Value::String("value".into()));
        assert_eq!(parsed["num"], Value::Number(42.into()));
    }

    #[tokio::test]
    async fn test_transform_builtin_unique() {
        let mut transforms = HashMap::new();
        transforms.insert("dedup".to_string(), r#"
            let arr = [1, 2, 3, 2, 1, 4];
            #{
                "result": unique(arr)
            }
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "dedup", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        let arr = result.get("result").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0], Value::Number(1.into()));
        assert_eq!(arr[1], Value::Number(2.into()));
        assert_eq!(arr[2], Value::Number(3.into()));
        assert_eq!(arr[3], Value::Number(4.into()));
    }

    #[tokio::test]
    async fn test_transform_script_not_found() {
        let config = make_config_with_transforms(HashMap::new());
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "nonexistent", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("not found"));
    }

    #[tokio::test]
    async fn test_transform_script_runtime_error() {
        let mut transforms = HashMap::new();
        transforms.insert("bad".to_string(), r#"
            let x = 1 / 0;
            x
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "bad", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("execution failed"));
    }

    #[tokio::test]
    async fn test_transform_vars_aggregation() {
        let mut transforms = HashMap::new();
        transforms.insert("collect".to_string(), r#"
            // _vars contains all injected variables as a map
            let encoded = json_encode(_vars);
            #{
                "json_string": encoded,
                "json_object": _vars
            }
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let mut state = ExecutionState::new(HashMap::new());
        state.write("start", {
            let mut m = HashMap::new();
            m.insert("city".to_string(), Value::String("Tokyo".to_string()));
            m.insert("count".to_string(), Value::Number(3.into()));
            m
        });

        let mut vars = HashMap::new();
        vars.insert("destination".to_string(), VariableBinding::Single("start.city".to_string()));
        vars.insert("days".to_string(), VariableBinding::Single("start.count".to_string()));
        let node = make_transform_node("t1", "collect", vars);

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();

        // json_object should be a map with destination and days
        let obj = result.get("json_object").unwrap();
        assert_eq!(obj.get("destination").unwrap(), &Value::String("Tokyo".into()));
        assert_eq!(obj.get("days").unwrap(), &Value::Number(3.into()));

        // json_string should be valid JSON containing both vars
        let json_str = result.get("json_string").unwrap().as_str().unwrap();
        let parsed: Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed["destination"], Value::String("Tokyo".into()));
        assert_eq!(parsed["days"], Value::Number(3.into()));
    }

    #[tokio::test]
    async fn test_transform_vars_empty() {
        let mut transforms = HashMap::new();
        transforms.insert("empty".to_string(), r#"
            _vars.len()
        "#.to_string());

        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());
        let node = make_transform_node("t1", "empty", HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        assert_eq!(result.get("result").unwrap(), &Value::Number(0.into()));
    }

    #[tokio::test]
    async fn test_transform_missing_file_and_script_config() {
        let config = make_config_with_transforms(HashMap::new());
        let state = ExecutionState::new(HashMap::new());
        let node = YamlNode {
            id: "t1".to_string(),
            node_type: NodeType::Transform,
            config: NodeConfig::default(),
        };

        let result = TransformExecutor.execute(&node, &state, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("script") || err.contains("file"));
    }

    // ---- Inline script tests ----

    fn make_inline_transform_node(id: &str, script: &str, variables: HashMap<String, VariableBinding>) -> YamlNode {
        YamlNode {
            id: id.to_string(),
            node_type: NodeType::Transform,
            config: NodeConfig {
                script: Some(script.to_string()),
                variables,
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn test_transform_inline_script() {
        let config = ExecutionConfig::default(); // no transforms_dir needed
        let state = ExecutionState::new(HashMap::new());
        let node = make_inline_transform_node("t1", r#"
            #{
                "greeting": "hello inline",
                "count": 99
            }
        "#, HashMap::new());

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        assert_eq!(result.get("greeting").unwrap(), &Value::String("hello inline".into()));
        assert_eq!(result.get("count").unwrap(), &Value::Number(99.into()));
    }

    #[tokio::test]
    async fn test_transform_inline_script_with_variables() {
        let config = ExecutionConfig::default();
        let mut state = ExecutionState::new(HashMap::new());
        state.write("start", {
            let mut m = HashMap::new();
            m.insert("name".to_string(), Value::String("World".to_string()));
            m
        });

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), VariableBinding::Single("start.name".to_string()));
        let node = make_inline_transform_node("t1", r#"
            #{
                "message": "Hello, " + name
            }
        "#, vars);

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        assert_eq!(result.get("message").unwrap(), &Value::String("Hello, World".into()));
    }

    #[tokio::test]
    async fn test_transform_inline_script_overrides_file() {
        // When both `script` and `file` are present, `script` wins
        let mut transforms = HashMap::new();
        transforms.insert("from_file".to_string(), r#"#{ "source": "file" }"#.to_string());
        let config = make_config_with_transforms(transforms);
        let state = ExecutionState::new(HashMap::new());

        let node = YamlNode {
            id: "t1".to_string(),
            node_type: NodeType::Transform,
            config: NodeConfig {
                file: Some("from_file".to_string()),
                script: Some(r#"#{ "source": "inline" }"#.to_string()),
                ..Default::default()
            },
        };

        let result = TransformExecutor.execute(&node, &state, &config).await.unwrap();
        assert_eq!(result.get("source").unwrap(), &Value::String("inline".into()));
    }
}
