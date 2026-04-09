//! Generate node executor - LLM calls

use std::collections::HashMap;
use async_trait::async_trait;
use handlebars::Handlebars;
use serde_json::{json, Value};
use tracing::{debug, info, warn, error};

use crate::schema::{YamlNode, TemplateSelector};
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use crate::expression::evaluate_expression;
use crate::providers::{ChatOptions, Message, MessageContent, ContentPart, StreamCallback};
use super::NodeExecutor;

/// Executor for generate nodes
pub struct GenerateExecutor;

impl GenerateExecutor {
    /// Resolve template configuration to get the final template name
    ///
    /// For simple config, returns the template name directly.
    /// For conditional config, evaluates conditions in order and returns the first match,
    /// or the default template if no conditions match.
    fn resolve_template_name(
        template_config: &TemplateSelector,
        state: &ExecutionState,
        node_id: &str,
    ) -> String {
        match template_config {
            TemplateSelector::Simple(name) => name.clone(),
            TemplateSelector::Conditional { default, conditions } => {
                for cond in conditions {
                    match evaluate_expression(&cond.when, state) {
                        Ok(true) => {
                            info!(
                                node = %node_id,
                                condition = %cond.when,
                                template = %cond.use_template,
                                "Template condition matched"
                            );
                            return cond.use_template.clone();
                        }
                        Ok(false) => {
                            debug!(
                                node = %node_id,
                                condition = %cond.when,
                                "Template condition not matched, trying next"
                            );
                        }
                        Err(e) => {
                            warn!(
                                node = %node_id,
                                condition = %cond.when,
                                error = %e,
                                "Template condition evaluation failed, skipping"
                            );
                        }
                    }
                }
                // No condition matched, use default
                info!(
                    node = %node_id,
                    template = %default,
                    "Using default template (no conditions matched)"
                );
                default.clone()
            }
        }
    }
}

#[async_trait]
impl NodeExecutor for GenerateExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_config = &node.config;
        let node_id = &node.id;

        debug!(
            node = %node_id,
            "Starting generate node execution"
        );

        // Get provider and model
        let provider_id = node_config.provider.as_ref()
            .ok_or_else(|| {
                error!(node = %node_id, "Generate node requires provider but none specified");
                EngineError::NodeExecutionFailed(
                    format!("Node '{}': Generate node requires provider", node_id)
                )
            })?;
        let model_id = node_config.model.as_ref()
            .ok_or_else(|| {
                error!(node = %node_id, provider = %provider_id, "Generate node requires model but none specified");
                EngineError::NodeExecutionFailed(
                    format!("Node '{}': Generate node requires model", node_id)
                )
            })?;

        // Get provider from registry
        let provider = config.providers.get(provider_id)
            .ok_or_else(|| {
                error!(
                    node = %node_id,
                    provider = %provider_id,
                    "Provider not found in registry"
                );
                EngineError::ProviderNotFound(provider_id.clone())
            })?;

        // Resolve template name (supports conditional selection)
        let template_name = node_config.template.as_ref()
            .map(|tc| Self::resolve_template_name(tc, state, node_id));

        // Get template if specified
        let template = template_name.as_ref()
            .and_then(|name| {
                let t = config.templates.get(name);
                if t.is_none() {
                    warn!(
                        node = %node_id,
                        template = %name,
                        available_templates = ?config.templates.keys().collect::<Vec<_>>(),
                        "Template not found, will use variables as prompt"
                    );
                }
                t
            });

        // Resolve variables from state (shared logic handles literals + state paths)
        let variables = super::resolve_variables(&node_config.variables, state);

        // Build prompt
        let (user_prompt, system_prompt, output_schema) = if let Some(tpl) = template {
            let user = render_template(&tpl.user_prompt, &variables);
            let template_system = tpl.system_prompt.as_ref()
                .map(|s| render_template(s, &variables));

            // Compose system_prompt: harness instructions + template system_prompt
            let system = compose_system_prompt(
                config.harness_instructions.as_deref(),
                None, // Reserved for skill metadata (Phase 2)
                template_system.as_deref(),
            );

            (user, system, Some(&tpl.output_schema))
        } else {
            // No template - use variables as prompt
            let prompt = variables.iter()
                .map(|(k, v)| format!("{}: {}", k, value_to_string(v)))
                .collect::<Vec<_>>()
                .join("\n");
            // Still inject harness even without template
            let system = compose_system_prompt(
                config.harness_instructions.as_deref(),
                None,
                None,
            );
            (if prompt.is_empty() { "Hello".into() } else { prompt }, system, None)
        };

        // Build final prompt (append JSON schema if defined)
        let final_prompt = if let Some(schema) = output_schema {
            if !schema.is_empty() {
                let example_json = generate_example_json(schema);
                format!("{}\n\nRespond in JSON format:\n{}", user_prompt, example_json)
            } else {
                user_prompt
            }
        } else {
            user_prompt
        };

        // Build messages
        let mut messages = Vec::new();

        if let Some(ref sys) = system_prompt {
            messages.push(Message::system(sys.clone()));
        }

        // Resolve images from explicit config.images bindings
        let mut image_parts = Vec::new();
        for binding in &node_config.images {
            if let Some(Value::String(data)) = super::resolve_single_binding(binding, state) {
                if data.is_empty() { continue; }
                if let Some(part) = super::parse_image_input(&data) {
                    image_parts.push(part);
                }
            }
        }

        // Build the user message (multimodal if images present)
        let has_images = !image_parts.is_empty();
        let user_message = if !has_images {
            // Pure text message (backward compatible)
            Message::user(final_prompt.clone())
        } else {
            // Multimodal message with text and images
            let mut content_parts = vec![ContentPart::Text { text: final_prompt.clone() }];
            content_parts.extend(image_parts);

            Message {
                role: crate::providers::MessageRole::User,
                content: MessageContent::Parts(content_parts),
                tool_call_id: None,
                tool_name: None,
                tool_calls: None,
            }
        };

        messages.push(user_message);

        // Build chat options
        let mut options = ChatOptions::new()
            .with_model(model_id);

        // Apply template config with capability-aware masking
        let model_caps = config.model_registry.get_capabilities(provider_id, model_id);
        let sampling_config = config.model_registry.get_sampling(provider_id);

        if let Some(tpl) = template {
            // Temperature: clamp to model/provider max
            let max_temp = config.model_registry.max_temperature(provider_id, model_id);
            let temp = tpl.config.temperature.min(max_temp);
            options = options.with_temperature(temp);
            options = options.with_max_tokens(tpl.config.max_tokens);

            // Top P: only if provider supports it
            if let Some(top_p) = tpl.config.top_p {
                let supports = sampling_config.map(|s| s.supports_top_p).unwrap_or(true);
                if supports {
                    // Check if both temp and top_p can coexist
                    let allow_both = sampling_config.map(|s| s.allow_both_temp_and_top_p).unwrap_or(true);
                    if allow_both || tpl.config.temperature == 0.0 {
                        options = options.with_top_p(top_p);
                    } else {
                        debug!(node = %node_id, "Skipping top_p: provider doesn't allow both temperature and top_p");
                    }
                } else {
                    debug!(node = %node_id, "Skipping top_p: provider doesn't support it");
                }
            }

            // Top K: only if provider supports it
            if let Some(top_k) = tpl.config.top_k {
                let supports = sampling_config.map(|s| s.supports_top_k).unwrap_or(false);
                if supports {
                    options = options.with_top_k(top_k);
                } else {
                    debug!(node = %node_id, "Skipping top_k: provider doesn't support it");
                }
            }

            // Thinking budget: only if model supports thinking
            if let Some(budget) = tpl.config.thinking_budget {
                let supports = model_caps.map(|c| c.thinking != "none").unwrap_or(true);
                if supports {
                    options = options.with_thinking_budget(budget);
                } else {
                    debug!(node = %node_id, "Skipping thinking_budget: model doesn't support thinking");
                }
            }

            // Reasoning effort
            if let Some(ref effort) = tpl.config.reasoning_effort {
                let parsed = match effort.as_str() {
                    "low" => crate::providers::ReasoningEffort::Low,
                    "high" => crate::providers::ReasoningEffort::High,
                    _ => crate::providers::ReasoningEffort::Medium,
                };
                options = options.with_reasoning_effort(parsed);
            }

            if tpl.config.enable_search {
                options = options.with_enable_search(true);
            }
        }

        // JSON output: only if model supports it, otherwise rely on prompt + robust extraction
        let has_schema = output_schema.map(|s| !s.is_empty()).unwrap_or(false);
        if has_schema {
            let supports_json = model_caps.map(|c| c.json).unwrap_or(true);
            if supports_json {
                // Check thinking + JSON compatibility
                let has_thinking = options.thinking_budget.is_some();
                if has_thinking {
                    let compat = model_caps
                        .map(|c| &c.thinking_json_compat)
                        .unwrap_or(&crate::providers::model_registry::ThinkingJsonCompat::Full);
                    match compat {
                        crate::providers::model_registry::ThinkingJsonCompat::None => {
                            // Incompatible: prefer JSON, drop thinking
                            debug!(node = %node_id, "Thinking+JSON incompatible: disabling thinking, keeping JSON");
                            options.thinking_budget = None;
                            options = options.with_json_output();
                        }
                        _ => {
                            // Full or Partial: enable both
                            options = options.with_json_output();
                        }
                    }
                } else {
                    options = options.with_json_output();
                }
            } else {
                debug!(
                    node = %node_id,
                    provider = %provider_id,
                    model = %model_id,
                    "Model doesn't support JSON output mode, relying on prompt injection + robust extraction"
                );
                // Don't set json_output — prompt already has "Respond in JSON format: {...}"
            }
        }

        // Tool definitions: only if model supports function calling
        if !node_config.tools.is_empty() {
            let supports_fc = model_caps.map(|c| c.function_calling).unwrap_or(true);
            if supports_fc {
                let tool_defs: Vec<crate::providers::ToolDefinition> = node_config.tools.iter()
                    .map(|t| crate::providers::ToolDefinition {
                        name: t.name.clone(),
                        description: t.description.clone().unwrap_or_default(),
                        parameters: t.parameters.clone().unwrap_or(serde_json::json!({
                            "type": "object",
                            "properties": {},
                        })),
                    })
                    .collect();
                options = options.with_tools(tool_defs);
            } else {
                warn!(
                    node = %node_id,
                    provider = %provider_id,
                    model = %model_id,
                    "Model doesn't support function calling, tools will not be available"
                );
            }
        }

        debug!(
            node = %node_id,
            provider = %provider_id,
            model = %model_id,
            template = template_name.as_deref().unwrap_or("(none)"),
            has_images = has_images,
            "Making LLM API call"
        );

        // Determine if we should stream
        // Streaming requires: 1) node config opt-in, 2) stream handler present,
        // 3) no JSON schema (JSON mode needs complete response), 4) no tools (tool calling needs full response)
        let has_json_schema = output_schema.map(|s| !s.is_empty()).unwrap_or(false);
        let has_tools = options.tools.is_some();
        let should_stream = node_config.stream
            && config.stream_handler.is_some()
            && !has_json_schema
            && !has_tools;

        // Make the API call (streaming or non-streaming, with optional tool calling loop)
        // Returns: (content, thinking, usage, ttft)
        let (content, thinking, usage, ttft) = if should_stream {
            // Streaming path (no tool calling)
            let handler = config.stream_handler.as_ref().unwrap().clone();
            let node_id_owned = node_id.clone();
            let callback: StreamCallback = Box::new(move |chunk: &str, accumulated: &str| {
                handler.on_text_chunk(&node_id_owned, chunk, accumulated);
            });

            match provider.stream(messages, options, callback).await {
                Ok(r) => (r.content, None, r.usage, r.ttft),
                Err(e) => {
                    error!(node = %node_id, error = %e, "Streaming LLM call failed");
                    return Err(EngineError::ProviderError(e));
                }
            }
        } else if has_tools {
            // Tool calling loop: LLM may request tool calls, we execute and feed back
            let max_rounds = node_config.max_tool_rounds.unwrap_or(10);
            let mut conversation = messages;
            let mut total_usage = crate::providers::TokenUsage::default();
            let mut final_content = String::new();
            let mut final_thinking = None;

            for round in 0..max_rounds {
                let response = provider.chat(conversation.clone(), options.clone()).await
                    .map_err(|e| {
                        error!(node = %node_id, round = round, error = %e, "Tool calling LLM call failed");
                        EngineError::ProviderError(e)
                    })?;

                // Accumulate usage
                total_usage.input_tokens += response.usage.input_tokens;
                total_usage.output_tokens += response.usage.output_tokens;
                total_usage.total_tokens += response.usage.total_tokens;

                if let Some(ref tool_calls) = response.tool_calls {
                    if !tool_calls.is_empty() {
                        debug!(
                            node = %node_id,
                            round = round,
                            tool_calls = tool_calls.len(),
                            tools = ?tool_calls.iter().map(|tc| &tc.name).collect::<Vec<_>>(),
                            "LLM requested tool calls"
                        );

                        // Add assistant message with tool calls to conversation
                        conversation.push(Message {
                            role: crate::providers::MessageRole::Assistant,
                            content: crate::providers::MessageContent::Text(response.content.clone()),
                            tool_call_id: None,
                            tool_name: None,
                            tool_calls: Some(tool_calls.clone()),
                        });

                        // Execute each tool call via subgraph
                        for tc in tool_calls {
                            let tool_ref = node_config.tools.iter()
                                .find(|t| t.name == tc.name);

                            let result_content = if let Some(tool) = tool_ref {
                                // Parse arguments as input HashMap
                                let input: HashMap<String, Value> = match &tc.arguments {
                                    Value::Object(obj) => obj.iter()
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect(),
                                    other => {
                                        warn!(
                                            node = %node_id,
                                            tool = %tc.name,
                                            arguments = ?other,
                                            "Tool call arguments is not an object, using empty input"
                                        );
                                        HashMap::new()
                                    }
                                };

                                // Execute subgraph
                                if let Some(subgraph) = config.subgraphs.get(&tool.subgraph_ref) {
                                    match super::subgraph_runner::execute(subgraph, input, config).await {
                                        Ok(result) => {
                                            total_usage.input_tokens += result.usage.input_tokens;
                                            total_usage.output_tokens += result.usage.output_tokens;
                                            serde_json::to_string(&result.outputs).unwrap_or_default()
                                        }
                                        Err(e) => format!("Tool execution error: {}", e),
                                    }
                                } else {
                                    format!("Subgraph '{}' not found", tool.subgraph_ref)
                                }
                            } else {
                                format!("Unknown tool: {}", tc.name)
                            };

                            // Add tool result to conversation (with name for Gemini compatibility)
                            conversation.push(Message::tool_result_with_name(&tc.id, &tc.name, result_content));
                        }

                        continue; // Next round — send tool results back to LLM
                    }
                }

                // No tool calls — LLM gave final response
                final_content = response.content;
                final_thinking = response.thinking;
                break;
            }

            (final_content, final_thinking, Some(total_usage), None)
        } else {
            // Simple non-streaming path (no tools)
            match provider.chat(messages, options).await {
                Ok(r) => {
                    debug!(
                        node = %node_id,
                        input_tokens = r.usage.input_tokens,
                        output_tokens = r.usage.output_tokens,
                        "LLM call completed"
                    );
                    (r.content, r.thinking, Some(r.usage), None)
                }
                Err(e) => {
                    error!(node = %node_id, error = %e, "LLM call failed");
                    return Err(EngineError::ProviderError(e));
                }
            }
        };

        // Build outputs
        let mut outputs = HashMap::new();

        // Try to parse JSON if we have a schema
        if let Some(schema) = output_schema {
            if !schema.is_empty() {
                // Try direct JSON parse first, then robust extraction
                let parsed = serde_json::from_str::<Value>(&content)
                    .or_else(|_| {
                        // LLM may wrap JSON in markdown, prose, or trailing text
                        if let Some(json_str) = super::extract_json_from_content(&content) {
                            serde_json::from_str::<Value>(&json_str)
                        } else {
                            serde_json::from_str::<Value>("")  // force Err
                        }
                    });

                match parsed {
                    Ok(parsed) => {
                        // Map schema fields to outputs
                        for field in schema {
                            if !field.name.is_empty() {
                                let value = parsed.get(&field.name)
                                    .cloned()
                                    .unwrap_or_else(|| get_default_for_type(&field.field_type));
                                outputs.insert(field.name.clone(), value);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            node = %node_id,
                            error = %e,
                            raw_content_preview = %content.chars().take(200).collect::<String>(),
                            "Failed to parse JSON response, using raw content"
                        );
                        outputs.insert("output".to_string(), Value::String(content.clone()));
                    }
                }
            } else {
                outputs.insert("output".to_string(), Value::String(content.clone()));
            }
        } else {
            outputs.insert("output".to_string(), Value::String(content.clone()));
        }

        // Add thinking if present (chat path only, streaming does not return thinking)
        if let Some(thinking) = thinking {
            outputs.insert("_thinking".to_string(), Value::String(thinking));
        }

        // Add TTFT if present (streaming path only)
        if let Some(ttft_ms) = ttft {
            outputs.insert("_ttft_ms".to_string(), json!(ttft_ms));
        }

        // Add usage info (including provider/model for cost tracking)
        if let Some(ref usage) = usage {
            let mut usage_json = json!({
                "provider": provider_id,
                "model": model_id,
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
            });
            if let Some(count) = usage.search_count {
                usage_json["search_count"] = json!(count);
            }
            outputs.insert("_usage".to_string(), usage_json);
        }

        // Add actual prompt for debugging (legacy field, unrendered template)
        outputs.insert("_prompt".to_string(), Value::String(final_prompt.clone()));

        // Capture actual rendered prompts if enabled (for observability)
        if config.capture_actual_prompt {
            if let Some(sys) = &system_prompt {
                outputs.insert("_actual_system_prompt".to_string(), Value::String(sys.clone()));
            }
            outputs.insert("_actual_user_prompt".to_string(), Value::String(final_prompt));
        }

        // Capture raw response if enabled (for observability)
        if config.capture_raw_response {
            outputs.insert("_raw_response".to_string(), Value::String(content));
        }

        Ok(outputs)
    }
}

/// Render a template using Handlebars (supports {{#if}}, {{#each}}, etc.)
/// Compose the final system_prompt from multiple layers:
/// harness instructions (highest priority) → skill metadata → template system_prompt
fn compose_system_prompt(
    harness: Option<&str>,
    skills: Option<&str>,
    template: Option<&str>,
) -> Option<String> {
    let parts: Vec<&str> = [harness, skills, template]
        .iter()
        .filter_map(|p| *p)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n---\n\n"))
    }
}

fn render_template(template: &str, variables: &HashMap<String, Value>) -> String {
    let reg = Handlebars::new();

    // Convert HashMap to Value::Object for Handlebars
    let context: Value = serde_json::to_value(variables).unwrap_or(Value::Object(Default::default()));

    match reg.render_template(template, &context) {
        Ok(rendered) => rendered,
        Err(e) => {
            warn!(
                error = %e,
                template_preview = %template.chars().take(100).collect::<String>(),
                available_vars = ?variables.keys().collect::<Vec<_>>(),
                "Handlebars template render failed, falling back to simple substitution"
            );
            // Fallback to simple substitution
            let mut result = template.to_string();
            for (name, value) in variables {
                let pattern = format!("{{{{{}}}}}", name);
                let replacement = value_to_string(value);
                result = result.replace(&pattern, &replacement);
            }
            result
        }
    }
}

/// Convert a Value to string for template substitution
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

/// Generate example JSON from output schema (supports nested children)
fn generate_example_json(schema: &[crate::schema::OutputField]) -> String {
    let obj = build_example_object(schema);
    serde_json::to_string_pretty(&obj).unwrap_or_default()
}

/// Recursively build example JSON object from schema fields
fn build_example_object(schema: &[crate::schema::OutputField]) -> Value {
    let mut obj = serde_json::Map::new();

    for field in schema {
        if field.name.is_empty() {
            continue;
        }

        let example = if let Some(ex) = &field.example {
            serde_json::from_str(ex).unwrap_or(Value::String(ex.clone()))
        } else if !field.children.is_empty() {
            // Has nested children — build recursively
            let child_obj = build_example_object(&field.children);
            if field.field_type.ends_with("[]") {
                json!([child_obj])
            } else {
                child_obj
            }
        } else {
            get_example_for_type(&field.field_type)
        };

        obj.insert(field.name.clone(), example);
    }

    Value::Object(obj)
}

/// Get example value for a field type
fn get_example_for_type(field_type: &str) -> Value {
    match field_type {
        "string" => Value::String("...".to_string()),
        "number" => json!(0),
        "boolean" => json!(false),
        "string[]" => json!(["..."]),
        "number[]" => json!([0]),
        "object" => json!({}),
        "object[]" => json!([{}]),
        _ => Value::String("...".to_string()),
    }
}

/// Get default value for a field type
fn get_default_for_type(field_type: &str) -> Value {
    match field_type {
        "string" => Value::String(String::new()),
        "number" => json!(0),
        "boolean" => json!(false),
        "string[]" | "number[]" | "object[]" => json!([]),
        "object" => json!({}),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_template() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), Value::String("Alice".to_string()));
        vars.insert("count".to_string(), json!(42));

        let result = render_template("Hello {{name}}, count is {{count}}", &vars);
        assert_eq!(result, "Hello Alice, count is 42");
    }

    #[test]
    fn test_generate_example_json() {
        use crate::schema::OutputField;

        let schema = vec![
            OutputField {
                name: "answer".to_string(),
                field_type: "string".to_string(),
                description: None,
                example: None,
                children: Vec::new(),
            },
            OutputField {
                name: "confidence".to_string(),
                field_type: "number".to_string(),
                description: None,
                example: Some("0.95".to_string()),
                children: Vec::new(),
            },
        ];

        let json = generate_example_json(&schema);
        assert!(json.contains("answer"));
        assert!(json.contains("confidence"));
    }
}
