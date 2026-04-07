//! Supervisor node executor - dynamic task planning and execution
//!
//! The Supervisor node implements a planning loop:
//! 1. Call the Planner LLM to generate todos
//! 2. Execute each tool (subgraph) based on the plan
//! 3. Collect results and update context
//! 4. Repeat until exit condition is met
//! 5. Return task_result and exit_reason

use std::collections::HashMap;
use std::time::Instant;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::schema::{YamlNode, ExitReason, Template, VariableBinding};
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use crate::providers::{ChatOptions, Message};
use super::NodeExecutor;

/// Action status in Supervisor execution
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Pending,
    Running,
    Success,
    Error,
}

/// Token usage tracking for Supervisor
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SupervisorTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl SupervisorTokenUsage {
    pub fn add(&mut self, other: &SupervisorTokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }

    /// Add from LLMCallUsage (extracts only token counts)
    pub fn add_from_llm_call(&mut self, other: &LLMCallUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Token usage for a single LLM call (includes provider/model for cost tracking)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LLMCallUsage {
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl LLMCallUsage {
    pub fn new(provider: String, model: String, input: u64, output: u64) -> Self {
        Self {
            provider,
            model,
            input_tokens: input,
            output_tokens: output,
        }
    }
}

/// Subgraph node execution record for observability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubgraphNodeExecution {
    pub node_id: String,
    /// Token usage with provider/model for this single LLM call
    pub usage: LLMCallUsage,
    /// Execution time in milliseconds
    pub timing_ms: u64,
    pub outputs: Value,
}

/// A single action execution record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionExecution {
    /// Short ID (e.g., "1a", "1b", "2a")
    pub short_id: String,
    /// Action ID from Planner
    pub action_id: String,
    /// Which iteration this action belongs to
    pub iteration: u32,
    /// Tool name (action_called)
    pub tool_name: String,
    /// Subgraph reference ID
    pub tool_ref: String,
    /// Input parameters
    pub inputs: Value,
    /// Output results (final output from subgraph)
    pub outputs: Option<Value>,
    /// Token usage for this action (sum of all subgraph nodes)
    pub usage: SupervisorTokenUsage,
    /// Detailed execution of each subgraph node
    pub subgraph_executions: Vec<SubgraphNodeExecution>,
    /// Execution status
    pub status: ActionStatus,
    /// Error message if failed
    pub error: Option<String>,
    /// Retry count
    pub retry_count: u32,
    /// Execution time in milliseconds
    pub timing_ms: u64,
}

/// Todo item from Planner
#[derive(Debug, Clone, Deserialize)]
pub struct TodoItem {
    pub action_id: String,
    pub action_called: String,
    #[serde(default)]
    pub action_prerequest: String,
    #[serde(default)]
    pub action_detail: Value,
    #[serde(default)]
    #[allow(dead_code)]
    pub action_layer: String,
}

/// Supervisor execution state
pub struct SupervisorState {
    pub iteration: u32,
    pub start_time: Instant,
    pub total_result_length: u64,
    pub tool_failure_count: u32,
    pub actions: Vec<ActionExecution>,
    pub exit_reason: Option<ExitReason>,
    /// Token usage from Planner call (includes provider/model)
    pub planner_usage: LLMCallUsage,
    /// Planner execution time in milliseconds
    pub planner_timing_ms: u64,
    /// Total token usage (Planner + all actions) - aggregated without provider/model
    pub total_usage: SupervisorTokenUsage,
}

impl SupervisorState {
    pub fn new() -> Self {
        Self {
            iteration: 0,
            start_time: Instant::now(),
            total_result_length: 0,
            tool_failure_count: 0,
            actions: Vec::new(),
            exit_reason: None,
            planner_usage: LLMCallUsage::default(),
            planner_timing_ms: 0,
            total_usage: SupervisorTokenUsage::default(),
        }
    }
}

/// Task result summary
#[derive(Debug, Clone, Serialize)]
pub struct TaskResult {
    pub summary: String,
    /// Token usage for the Planner LLM call (includes provider/model)
    pub planner_usage: LLMCallUsage,
    /// Planner execution time in milliseconds
    pub planner_timing_ms: u64,
    /// All action executions
    pub actions: Vec<ActionExecution>,
    /// Statistics including token usage
    pub stats: TaskStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskStats {
    pub total: u32,
    pub success: u32,
    pub failed: u32,
    pub iterations: u32,
    /// Total token usage across Planner + all actions
    pub token_usage: SupervisorTokenUsage,
}

/// Supervisor node executor
pub struct SupervisorExecutor;

#[async_trait]
impl NodeExecutor for SupervisorExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_config = &node.config;

        // Get Supervisor configuration
        let max_iterations = node_config.max_iterations.unwrap_or(10);
        let max_retry_per_tool = node_config.max_retry_per_tool.unwrap_or(3);
        let timeout_ms = node_config.timeout.unwrap_or(300_000); // 5 minutes default
        let max_result_length = node_config.max_result_length.unwrap_or(1_000_000);
        let fail_on_tool_count = node_config.fail_on_tool_count;
        let parallel = node_config.parallel.unwrap_or(false);

        // Get provider and model
        let provider_id = node_config.provider.as_ref()
            .ok_or_else(|| EngineError::NodeExecutionFailed("Supervisor requires provider".into()))?;
        let model_id = node_config.model.as_ref()
            .ok_or_else(|| EngineError::NodeExecutionFailed("Supervisor requires model".into()))?;

        // Get planner template
        let template_selector = node_config.template.as_ref()
            .ok_or_else(|| EngineError::NodeExecutionFailed("Supervisor requires template".into()))?;
        // For Supervisor, we use the default template (no conditional logic)
        let template_name = template_selector.default_template();
        let template = config.templates.get(template_name)
            .ok_or_else(|| EngineError::TemplateNotFound(template_name.to_string()))?;

        // Build tool lookup: tool_name -> (subgraph_ref, description)
        let mut tool_lookup: HashMap<String, (String, Option<String>)> = HashMap::new();
        for tool_ref in &node_config.tools {
            tool_lookup.insert(
                tool_ref.name.clone(),
                (tool_ref.subgraph_ref.clone(), tool_ref.description.clone()),
            );
        }

        // Build tools description for Planner prompt
        let tools_description = build_tools_description(&node_config.tools);

        // Initialize supervisor state
        let mut sup_state = SupervisorState::new();

        // Resolve user_input variable from state
        let user_input = resolve_user_input(state, &node_config.variables);

        // Step 1: Call Planner ONCE to generate all todos (with timing)
        let planner_start = Instant::now();
        let (todos, planner_usage) = call_planner(
            config,
            provider_id,
            model_id,
            template,
            &user_input,
            "No previous results.", // Initial context
            &tools_description,
        ).await?;
        let planner_timing_ms = planner_start.elapsed().as_millis() as u64;

        // Store planner usage and timing
        sup_state.planner_usage = planner_usage.clone();
        sup_state.planner_timing_ms = planner_timing_ms;
        sup_state.total_usage.add_from_llm_call(&planner_usage);

        // Check if todos are empty (nothing to do)
        if todos.is_empty() {
            sup_state.exit_reason = Some(ExitReason::TodoEmpty);
        } else {
            // Step 2: Build execution plan based on dependencies
            // For now, execute sequentially (TODO: respect action_prerequest for parallel execution)
            let mut pending_todos: Vec<(usize, &TodoItem)> = todos.iter().enumerate().collect();
            let mut completed_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

            // Step 3: Execute todos until all complete or exit condition
            while !pending_todos.is_empty() {
                sup_state.iteration += 1;

                // Check exit conditions
                if sup_state.iteration > max_iterations {
                    sup_state.exit_reason = Some(ExitReason::MaxIterations);
                    break;
                }

                if sup_state.start_time.elapsed().as_millis() as u64 > timeout_ms {
                    sup_state.exit_reason = Some(ExitReason::Timeout);
                    break;
                }

                if sup_state.total_result_length > max_result_length {
                    sup_state.exit_reason = Some(ExitReason::MaxResultLength);
                    break;
                }

                if let Some(threshold) = fail_on_tool_count {
                    if sup_state.tool_failure_count >= threshold {
                        sup_state.exit_reason = Some(ExitReason::ToolFailures);
                        break;
                    }
                }

                // Find todos ready to execute (dependencies satisfied)
                let ready_todos: Vec<(usize, &TodoItem)> = pending_todos.iter()
                    .filter(|(_, todo)| {
                        // Check if prerequest is satisfied
                        todo.action_prerequest.is_empty() ||
                        completed_ids.contains(&todo.action_prerequest)
                    })
                    .cloned()
                    .collect();

                if ready_todos.is_empty() {
                    // No todos ready but pending_todos not empty = dependency deadlock
                    tracing::warn!("Dependency deadlock detected, executing remaining todos");
                    // Force execute first pending
                    if let Some((idx, todo)) = pending_todos.first().cloned() {
                        let ready_todos = vec![(idx, todo)];
                        execute_ready_todos(
                            &ready_todos,
                            &tool_lookup,
                            config,
                            &mut sup_state,
                            &mut completed_ids,
                            &mut pending_todos,
                            max_retry_per_tool,
                        ).await;
                    }
                } else {
                    // Execute ready todos (parallel if enabled, otherwise sequential)
                    if parallel && ready_todos.len() > 1 {
                        // Execute all ready todos in parallel
                        execute_ready_todos(
                            &ready_todos,
                            &tool_lookup,
                            config,
                            &mut sup_state,
                            &mut completed_ids,
                            &mut pending_todos,
                            max_retry_per_tool,
                        ).await;
                    } else {
                        // Execute one at a time
                        if let Some((idx, todo)) = ready_todos.first().cloned() {
                            execute_ready_todos(
                                &[(idx, todo)],
                                &tool_lookup,
                                config,
                                &mut sup_state,
                                &mut completed_ids,
                                &mut pending_todos,
                                max_retry_per_tool,
                            ).await;
                        }
                    }
                }
            }

            // If we finished all todos normally
            if pending_todos.is_empty() && sup_state.exit_reason.is_none() {
                sup_state.exit_reason = Some(ExitReason::TodoEmpty);
            }
        }

        // Build final task_result
        let success_count = sup_state.actions.iter()
            .filter(|a| a.status == ActionStatus::Success)
            .count() as u32;
        let failed_count = sup_state.actions.iter()
            .filter(|a| a.status == ActionStatus::Error)
            .count() as u32;

        let task_result = TaskResult {
            summary: format!(
                "Completed {} actions ({} success, {} failed) in {} iterations",
                sup_state.actions.len(),
                success_count,
                failed_count,
                sup_state.iteration
            ),
            planner_usage: sup_state.planner_usage.clone(),
            planner_timing_ms: sup_state.planner_timing_ms,
            actions: sup_state.actions.clone(),
            stats: TaskStats {
                total: sup_state.actions.len() as u32,
                success: success_count,
                failed: failed_count,
                iterations: sup_state.iteration,
                token_usage: sup_state.total_usage.clone(),
            },
        };

        // Build outputs
        let mut outputs = HashMap::new();
        outputs.insert("task_result".to_string(), serde_json::to_value(&task_result)?);
        outputs.insert("exit_reason".to_string(), serde_json::to_value(
            sup_state.exit_reason.unwrap_or(ExitReason::Default)
        )?);

        // Add _usage for main engine token tracking
        outputs.insert("_usage".to_string(), json!({
            "input_tokens": sup_state.total_usage.input_tokens,
            "output_tokens": sup_state.total_usage.output_tokens,
        }));

        // Add individual action results for downstream access
        // Include failed actions with error info so downstream nodes can inspect
        for action in &sup_state.actions {
            let value = if let Some(ref action_outputs) = action.outputs {
                action_outputs.clone()
            } else {
                json!({
                    "_error": true,
                    "_error_message": action.error.clone().unwrap_or_default(),
                    "_tool": action.tool_name.clone(),
                })
            };
            outputs.insert(
                format!("action_{}", action.short_id),
                value,
            );
        }

        Ok(outputs)
    }
}

/// Build tools description for Planner prompt
fn build_tools_description(tools: &[crate::schema::ToolRef]) -> String {
    let mut lines = Vec::new();
    for tool in tools {
        let desc = tool.description.as_deref().unwrap_or("No description");
        lines.push(format!("- {}: {}", tool.name, desc));
    }
    lines.join("\n")
}

/// Resolve user_input variable from state
fn resolve_user_input(state: &ExecutionState, variables: &HashMap<String, VariableBinding>) -> String {
    // Try to find user_input in variables
    if let Some(binding) = variables.get("user_input") {
        // For array bindings, use the first source that has a value
        for path in binding.sources() {
            if path.is_empty() { continue; }
            if let Some(value) = state.read(path) {
                if !value.is_null() {
                    return value_to_string(&value);
                }
            }
        }
    }
    // Fallback: try input.user_input
    if let Some(value) = state.read("input.user_input") {
        return value_to_string(&value);
    }
    "".to_string()
}

/// Call the Planner LLM to generate todos
async fn call_planner(
    config: &ExecutionConfig,
    provider_id: &str,
    model_id: &str,
    template: &Template,
    user_input: &str,
    context: &str,
    tools: &str,
) -> Result<(Vec<TodoItem>, LLMCallUsage), EngineError> {
    let provider = config.providers.get(provider_id)
        .ok_or_else(|| EngineError::ProviderNotFound(provider_id.to_string()))?;

    // Build variables for template
    let mut variables: HashMap<String, Value> = HashMap::new();
    variables.insert("user_input".to_string(), Value::String(user_input.to_string()));
    variables.insert("context".to_string(), Value::String(context.to_string()));
    variables.insert("tools".to_string(), Value::String(tools.to_string()));

    // Render prompt
    let user_prompt = render_template(&template.user_prompt, &variables);
    let system_prompt = template.system_prompt.as_ref()
        .map(|s| render_template(s, &variables));

    // Build final prompt with JSON schema hint
    let final_prompt = format!(
        "{}\n\nRespond with a JSON object containing a \"todos\" array. Each todo should have: action_id, action_called, action_prerequest, action_detail, action_layer.",
        user_prompt
    );

    // Build messages
    let mut messages = Vec::new();
    if let Some(sys) = system_prompt {
        messages.push(Message::system(sys));
    }
    messages.push(Message::user(final_prompt));

    // Chat options
    let options = ChatOptions::new()
        .with_model(model_id)
        .with_temperature(template.config.temperature)
        .with_max_tokens(template.config.max_tokens)
        .with_json_output();

    // Make API call
    let response = provider.chat(messages, options).await?;

    // Extract token usage with provider/model for cost tracking
    let usage = LLMCallUsage::new(
        provider_id.to_string(),
        model_id.to_string(),
        response.usage.input_tokens,
        response.usage.output_tokens,
    );

    // Log planner response and usage
    tracing::debug!(
        provider = %usage.provider,
        model = %usage.model,
        input_tokens = usage.input_tokens,
        output_tokens = usage.output_tokens,
        response_preview = %response.content.chars().take(500).collect::<String>(),
        "Planner LLM response received"
    );

    // Parse response
    let todos = parse_planner_response(&response.content)?;
    Ok((todos, usage))
}

/// Parse Planner response to extract todos
fn parse_planner_response(content: &str) -> Result<Vec<TodoItem>, EngineError> {
    // Try to parse as JSON
    let parsed: Value = serde_json::from_str(content)
        .or_else(|_| {
            // Try to extract JSON from markdown code block
            extract_json_from_markdown(content)
                .and_then(|json| serde_json::from_str(&json).ok())
                .ok_or(EngineError::SupervisorPlannerFailed)
        })?;

    // Handle both formats:
    // 1. {"todos": [...]} - object with todos key
    // 2. [...] - direct array
    let todos_array = if parsed.is_array() {
        // Direct array format
        parsed.as_array().cloned().unwrap_or_default()
    } else if let Some(todos) = parsed.get("todos").and_then(|v| v.as_array()) {
        // Object with todos key
        todos.clone()
    } else {
        return Err(EngineError::SupervisorPlannerFailed);
    };

    let mut result = Vec::new();
    for todo in todos_array {
        if let Ok(item) = serde_json::from_value::<TodoItem>(todo.clone()) {
            result.push(item);
        }
    }

    Ok(result)
}

/// Extract JSON from markdown code block
fn extract_json_from_markdown(content: &str) -> Option<String> {
    // Try to find ```json ... ``` block
    if let Some(start) = content.find("```json") {
        let start = start + 7;
        if let Some(end) = content[start..].find("```") {
            return Some(content[start..start + end].trim().to_string());
        }
    }
    // Try plain ``` block
    if let Some(start) = content.find("```") {
        let start = start + 3;
        if let Some(end) = content[start..].find("```") {
            let json = content[start..start + end].trim();
            if json.starts_with('{') || json.starts_with('[') {
                return Some(json.to_string());
            }
        }
    }
    None
}

/// Execute ready todos with retry logic
async fn execute_ready_todos<'a>(
    ready_todos: &[(usize, &'a TodoItem)],
    tool_lookup: &HashMap<String, (String, Option<String>)>,
    config: &ExecutionConfig,
    sup_state: &mut SupervisorState,
    completed_ids: &mut std::collections::HashSet<String>,
    pending_todos: &mut Vec<(usize, &'a TodoItem)>,
    max_retry: u32,
) {
    for (idx, todo) in ready_todos {
        let short_id = generate_action_short_id(sup_state.actions.len());

        let (tool_ref, _) = tool_lookup.get(&todo.action_called)
            .map(|(r, d)| (r.clone(), d.clone()))
            .unwrap_or_default();

        // Execute with retry
        let mut last_error = None;
        let mut retry_count = 0u32;
        let mut final_result = None;
        let exec_start = Instant::now();

        for attempt in 0..=max_retry {
            if attempt > 0 {
                tracing::info!(
                    tool = %todo.action_called,
                    attempt = attempt,
                    max_retry = max_retry,
                    "Retrying tool execution"
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            }

            match execute_subgraph(todo, &tool_ref, config).await {
                Ok(result) => {
                    final_result = Some(result);
                    retry_count = attempt;
                    break;
                }
                Err(e) => {
                    tracing::warn!(
                        tool = %todo.action_called,
                        attempt = attempt,
                        error = %e,
                        "Tool execution failed"
                    );
                    last_error = Some(e);
                    retry_count = attempt;
                }
            }
        }

        let timing_ms = exec_start.elapsed().as_millis() as u64;

        let action = if let Some(subgraph_result) = final_result {
            let result_str = serde_json::to_string(&subgraph_result.outputs).unwrap_or_default();
            sup_state.total_result_length += result_str.len() as u64;
            sup_state.total_usage.add(&subgraph_result.usage);

            ActionExecution {
                short_id: short_id.clone(),
                action_id: todo.action_id.clone(),
                iteration: sup_state.iteration,
                tool_name: todo.action_called.clone(),
                tool_ref,
                inputs: todo.action_detail.clone(),
                outputs: Some(subgraph_result.outputs),
                usage: subgraph_result.usage,
                subgraph_executions: subgraph_result.executions,
                status: ActionStatus::Success,
                error: None,
                retry_count,
                timing_ms,
            }
        } else {
            sup_state.tool_failure_count += 1;
            let error_msg = last_error.map(|e| e.to_string()).unwrap_or_default();

            ActionExecution {
                short_id: short_id.clone(),
                action_id: todo.action_id.clone(),
                iteration: sup_state.iteration,
                tool_name: todo.action_called.clone(),
                tool_ref,
                inputs: todo.action_detail.clone(),
                outputs: None,
                usage: SupervisorTokenUsage::default(),
                subgraph_executions: Vec::new(),
                status: ActionStatus::Error,
                error: Some(error_msg),
                retry_count,
                timing_ms,
            }
        };

        sup_state.actions.push(action);
        completed_ids.insert(todo.action_id.clone());
        pending_todos.retain(|(i, _)| i != idx);
    }
}

/// Generate action short_id like "a", "b", ..., "z", "aa", "ab", ...
fn generate_action_short_id(index: usize) -> String {
    let mut result = String::new();
    let mut n = index;
    loop {
        result.insert(0, (b'a' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    result
}

/// Execute a subgraph (ToolBox) with the given input
/// Result of subgraph execution
struct SubgraphResult {
    /// Final outputs from the last node
    outputs: Value,
    /// Total token usage across all nodes
    usage: SupervisorTokenUsage,
    /// Execution details for each node
    executions: Vec<SubgraphNodeExecution>,
}

async fn execute_subgraph(
    todo: &TodoItem,
    subgraph_ref: &str,
    config: &ExecutionConfig,
) -> Result<SubgraphResult, EngineError> {
    let subgraph = config.subgraphs.get(subgraph_ref)
        .ok_or_else(|| EngineError::SupervisorToolNotFound(
            format!("Subgraph '{}' not found for tool '{}'", subgraph_ref, todo.action_called)
        ))?;

    tracing::debug!(
        tool = %todo.action_called,
        subgraph = %subgraph_ref,
        action_detail = ?todo.action_detail,
        "Executing subgraph for tool"
    );

    // Build input from action_detail
    let mut input: HashMap<String, Value> = HashMap::new();
    if let Value::Object(obj) = &todo.action_detail {
        for (key, value) in obj {
            input.insert(key.clone(), value.clone());
        }
    }

    // Delegate to shared subgraph runner
    let result = super::subgraph_runner::execute(subgraph, input, config).await?;

    tracing::debug!(
        subgraph = %subgraph_ref,
        total_input_tokens = result.usage.input_tokens,
        total_output_tokens = result.usage.output_tokens,
        execution_count = result.executions.len(),
        "Subgraph execution completed"
    );

    Ok(SubgraphResult {
        outputs: result.outputs,
        usage: result.usage,
        executions: result.executions,
    })
}


/// Render a template with variable substitution
fn render_template(template: &str, variables: &HashMap<String, Value>) -> String {
    let mut result = template.to_string();

    for (name, value) in variables {
        let pattern = format!("{{{{{}}}}}", name);
        let replacement = value_to_string(value);
        result = result.replace(&pattern, &replacement);
    }

    result
}

/// Convert a Value to string
fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => serde_json::to_string(arr).unwrap_or_default(),
        Value::Object(obj) => serde_json::to_string(obj).unwrap_or_default(),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_planner_response() {
        let json = r#"{"todos": [{"action_id": "test", "action_called": "tool1"}]}"#;
        let todos = parse_planner_response(json).unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].action_id, "test");
    }

    #[test]
    fn test_extract_json_from_markdown() {
        let content = r#"Here is the plan:
```json
{"todos": []}
```
"#;
        let json = extract_json_from_markdown(content).unwrap();
        assert!(json.contains("todos"));
    }

    #[test]
    fn test_build_tools_description() {
        let tools = vec![
            crate::schema::ToolRef {
                name: "search".to_string(),
                description: Some("Search the web".to_string()),
                subgraph_ref: "sg1".to_string(),
            },
        ];
        let desc = build_tools_description(&tools);
        assert!(desc.contains("search"));
        assert!(desc.contains("Search the web"));
    }
}
