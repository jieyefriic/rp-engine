//! Workflow node for sub-workflow execution
//!
//! This node allows workflows to call other workflows as sub-workflows,
//! enabling modular and reusable workflow compositions.
//!
//! # Example YAML
//! ```yaml
//! - id: compress_history
//!   type: workflow
//!   layer: 7
//!   config:
//!     workflow: compress_history        # Sub-workflow name
//!     variables:                        # Input mapping (param -> parent state path)
//!       user_id: start.user_id
//!       character_id: start.character_id
//!     output:                           # Output mapping (sub output -> parent field)
//!       was_compressed: was_compressed
//!       compressed_count: compressed_count
//! ```

use std::collections::HashMap;
use std::time::Instant;
use async_trait::async_trait;
use serde_json::Value;

use crate::nodes::CustomNode;
use crate::schema::NodeConfig;
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;

/// Workflow Node - executes a sub-workflow
///
/// Configuration:
/// - workflow: Sub-workflow name (required)
/// - variables.*: Input parameter mapping (param name -> parent state path)
/// - output.*: Output mapping (sub output name -> parent state field)
pub struct WorkflowNode;

impl WorkflowNode {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkflowNode {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CustomNode for WorkflowNode {
    fn node_type(&self) -> &str {
        "workflow"
    }

    async fn execute(
        &self,
        config: &NodeConfig,
        state: &ExecutionState,
        engine_config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        // 1. Get sub-workflow reference from config.workflow field
        let workflow_ref = config.workflow.as_ref()
            .ok_or_else(|| EngineError::MissingField(
                "workflow field is required for workflow node".into()
            ))?;

        tracing::info!(workflow = %workflow_ref, "Executing sub-workflow");

        // 2. Get sub-workflow definition
        let child_workflow = engine_config.get_workflow(workflow_ref)
            .ok_or_else(|| EngineError::WorkflowNotFound(workflow_ref.clone()))?;

        // 3. Build sub-workflow input from parent state
        //    Read values from parent state using config.variables mapping
        let mut child_input = super::resolve_variables(&config.variables, state);
        // Remove special variables not intended as sub-workflow input
        child_input.remove("capture_child_state");

        tracing::debug!(
            workflow = %workflow_ref,
            input_params = ?child_input.keys().collect::<Vec<_>>(),
            "Sub-workflow input prepared"
        );

        // 4. Execute sub-workflow (creates new isolated ExecutionState)
        let start_time = Instant::now();
        let result = execute_child_workflow(child_workflow, child_input, engine_config).await?;
        let timing_ms = start_time.elapsed().as_millis() as u64;

        tracing::info!(
            workflow = %workflow_ref,
            status = ?result.status,
            timing_ms = timing_ms,
            "Sub-workflow completed"
        );

        // 5. Build output (map sub-workflow outputs back to parent state)
        let mut outputs = HashMap::new();

        // 5a. Map config.output fields: sub-workflow response -> parent state
        for (output_name, child_field) in &config.output {
            if let Some(value) = result.output.get(child_field) {
                outputs.insert(output_name.clone(), value.clone());
            }
        }

        // 5b. Add execution metadata
        outputs.insert("_status".into(),
            Value::String(format!("{:?}", result.status)));
        outputs.insert("_timing_ms".into(),
            Value::Number(timing_ms.into()));

        if let Some(error) = result.error {
            outputs.insert("_error".into(), Value::String(error));
        }

        // 5c. Optional: capture full child state for debugging/observability
        let capture = config.variables.get("capture_child_state")
            .and_then(|v| {
                // Get first source - could be literal "true"/"false" or a state path
                v.first_source().and_then(|s| {
                    // Try to parse as literal bool first
                    if let Ok(b) = s.parse::<bool>() {
                        return Some(b);
                    }
                    // Otherwise try to resolve from state
                    state.read(s).and_then(|v| v.as_bool())
                })
            })
            .unwrap_or(false);
        if capture {
            outputs.insert("_child_executions".into(),
                serde_json::to_value(&result.executions).unwrap_or(Value::Null));
        }

        Ok(outputs)
    }
}

/// Execute a child workflow with isolated state
async fn execute_child_workflow(
    workflow: &crate::schema::PreparedWorkflow,
    input: HashMap<String, Value>,
    config: &ExecutionConfig,
) -> Result<crate::engine::ExecutionResult, EngineError> {
    use crate::engine::execute_workflow;

    // Create a new ExecutionConfig for the child workflow
    // This shares providers, templates, etc. but each execution gets its own state
    let child_config = ExecutionConfig {
        providers: config.providers.clone(),
        embedding_providers: config.embedding_providers.clone(),
        search_providers: config.search_providers.clone(),
        timeout: config.timeout,
        max_iterations: config.max_iterations,
        templates: config.templates.clone(),
        subgraphs: config.subgraphs.clone(),
        data_sources: config.data_sources.clone(),
        node_registry: config.node_registry.clone(),
        pools: config.pools.clone(),
        capture_actual_prompt: config.capture_actual_prompt,
        capture_raw_response: config.capture_raw_response,
        // Sub-workflows never carry their own yaml snapshot — the parent
        // workflow's yaml already references them, so embedding here would
        // be redundant and bloat result documents. The flag stays off
        // regardless of the parent's setting.
        capture_source_yaml: false,
        workflows: config.workflows.clone(),
        transforms: config.transforms.clone(),
        stream_handler: config.stream_handler.clone(),
        model_registry: config.model_registry.clone(),
        harness_instructions: config.harness_instructions.clone(),
        harness_memory_file: config.harness_memory_file.clone(),
        mcp_servers: config.mcp_servers.clone(),
    };

    // Execute the child workflow
    // execute_workflow creates a new ExecutionState internally
    execute_workflow(workflow, input, child_config, None).await
}
