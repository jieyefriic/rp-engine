//! Skill set node executor - provides skills for progressive disclosure in generate nodes
//!
//! The skill_set node resolves skill content (inline or from files) and writes
//! the resolved skills to execution state. Downstream generate nodes read these
//! skills to inject metadata into system_prompt and register load_skill/load_skill_reference
//! built-in tools for progressive disclosure.

use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

use crate::schema::YamlNode;
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::NodeExecutor;

/// Executor for skill_set nodes
pub struct SkillSetExecutor;

#[async_trait]
impl NodeExecutor for SkillSetExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        _state: &ExecutionState,
        _config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_id = &node.id;
        let skills = &node.config.skills;

        debug!(
            node = %node_id,
            skill_count = skills.len(),
            "Resolving skill set"
        );

        let mut resolved = Vec::new();

        for skill in skills {
            // Validate name
            if skill.name.is_empty() {
                return Err(EngineError::NodeExecutionFailed(
                    format!("Node '{}': skill has empty name", node_id)
                ));
            }

            // Resolve content: inline takes precedence over content_file
            let content = if let Some(ref c) = skill.content {
                c.clone()
            } else if let Some(ref path) = skill.content_file {
                match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(e) => {
                        return Err(EngineError::NodeExecutionFailed(
                            format!(
                                "Node '{}': failed to read content_file '{}' for skill '{}': {}",
                                node_id, path, skill.name, e
                            )
                        ));
                    }
                }
            } else {
                return Err(EngineError::NodeExecutionFailed(
                    format!(
                        "Node '{}': skill '{}' has neither content nor content_file",
                        node_id, skill.name
                    )
                ));
            };

            debug!(
                node = %node_id,
                skill = %skill.name,
                content_len = content.len(),
                references = skill.references.len(),
                "Resolved skill"
            );

            resolved.push(json!({
                "name": skill.name,
                "description": skill.description,
                "content": content,
                "references": skill.references,
            }));
        }

        let mut outputs = HashMap::new();
        let count = resolved.len();
        outputs.insert("skills".to_string(), Value::Array(resolved));
        outputs.insert("_skill_count".to_string(), json!(count));

        debug!(
            node = %node_id,
            skill_count = count,
            "Skill set resolved"
        );

        Ok(outputs)
    }
}
