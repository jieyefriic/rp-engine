//! Response node executor

use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::Value;

use crate::schema::YamlNode;
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::NodeExecutor;

/// Executor for response nodes
pub struct ResponseExecutor;

#[async_trait]
impl NodeExecutor for ResponseExecutor {
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

        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_response_mapping() {
        let mut state = ExecutionState::new(HashMap::new());

        // Simulate a generate node output
        let mut gen_outputs = HashMap::new();
        gen_outputs.insert("answer".to_string(), Value::String("42".to_string()));
        state.write("gen", gen_outputs);

        let config = ExecutionConfig::default();

        let mut output_map = HashMap::new();
        output_map.insert("result".to_string(), "gen.answer".to_string());

        let node = YamlNode {
            id: "response".to_string(),
            node_type: crate::schema::NodeType::Response,
            layer: 1,
            config: crate::schema::NodeConfig {
                output: output_map,
                ..Default::default()
            },
        };

        let executor = ResponseExecutor;
        let outputs = executor.execute(&node, &state, &config).await.unwrap();

        assert_eq!(outputs.get("result"), Some(&Value::String("42".to_string())));
    }
}
