//! Checkpoint node executor
//!
//! Behaves identically to ResponseExecutor (maps output fields from state),
//! but signals the execution engine to pause and return a CheckpointState
//! for later resume. The pause semantics are handled by the executor loop,
//! not by this executor itself.

use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::Value;

use crate::schema::YamlNode;
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::NodeExecutor;

/// Executor for checkpoint nodes
pub struct CheckpointExecutor;

#[async_trait]
impl NodeExecutor for CheckpointExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        _config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let mut outputs = HashMap::new();

        // Map output fields from config (same as ResponseExecutor)
        for (field, path) in &node.config.output {
            match state.read(path) {
                Some(value) => { outputs.insert(field.clone(), value); }
                None => {
                    tracing::debug!(
                        node = %node.id,
                        field = %field,
                        path = %path,
                        "Checkpoint field resolved to null (path not found in state)"
                    );
                    outputs.insert(field.clone(), Value::Null);
                }
            }
        }

        Ok(outputs)
    }
}
