//! Start node executor

use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::Value;

use crate::schema::YamlNode;
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::NodeExecutor;

/// Executor for start nodes
pub struct StartExecutor;

#[async_trait]
impl NodeExecutor for StartExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        _config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let mut outputs = HashMap::new();

        // Map output fields from config
        for (field, path) in &node.config.output {
            let value = state.read(path).unwrap_or(Value::Null);
            outputs.insert(field.clone(), value);
        }

        // If no outputs defined, pass through all inputs
        if outputs.is_empty() {
            for (key, value) in state.get_input() {
                outputs.insert(key.clone(), value.clone());
            }
        }

        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_start_passthrough() {
        let mut input = HashMap::new();
        input.insert("query".to_string(), Value::String("hello".to_string()));

        let state = ExecutionState::new(input);
        let config = ExecutionConfig::default();

        let node = YamlNode {
            id: "start".to_string(),
            node_type: crate::schema::NodeType::Start,

            config: Default::default(),
        };

        let executor = StartExecutor;
        let outputs = executor.execute(&node, &state, &config).await.unwrap();

        assert_eq!(outputs.get("query"), Some(&Value::String("hello".to_string())));
    }

    #[tokio::test]
    async fn test_start_with_mapping() {
        let mut input = HashMap::new();
        input.insert("user_query".to_string(), Value::String("test".to_string()));

        let state = ExecutionState::new(input);
        let config = ExecutionConfig::default();

        let mut output_map = HashMap::new();
        output_map.insert("query".to_string(), "input.user_query".to_string());

        let node = YamlNode {
            id: "start".to_string(),
            node_type: crate::schema::NodeType::Start,

            config: crate::schema::NodeConfig {
                output: output_map,
                ..Default::default()
            },
        };

        let executor = StartExecutor;
        let outputs = executor.execute(&node, &state, &config).await.unwrap();

        assert_eq!(outputs.get("query"), Some(&Value::String("test".to_string())));
    }
}
