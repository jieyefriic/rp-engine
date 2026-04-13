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
use crate::providers::{ChatOptions, Message, MessageContent, ContentPart, StreamCallback, ToolCall};
use crate::schema::NodeConfig;
use super::NodeExecutor;

/// Resolved skill from an upstream skill_set node (internal to generate)
struct ResolvedSkill {
    description: String,
    content: String,
    references: Vec<String>,
}

/// Resolved MCP tool from an upstream mcp_tools node (internal to generate)
struct ResolvedMcpTool {
    description: String,
    input_schema: Value,
    server_id: String,
}

/// Context for dispatching tool calls (built-in + subgraph + MCP)
struct ToolDispatchContext<'a> {
    node_config: &'a NodeConfig,
    config: &'a ExecutionConfig,
    skill_index: &'a HashMap<String, ResolvedSkill>,
    mcp_tool_index: &'a HashMap<String, ResolvedMcpTool>,
    node_id: &'a str,
}

/// Dispatch a single tool call to the appropriate handler.
/// Returns (result_content, optional_token_usage).
async fn dispatch_tool_call(
    tc: &ToolCall,
    ctx: &ToolDispatchContext<'_>,
) -> (String, Option<crate::providers::TokenUsage>) {
    match tc.name.as_str() {
        "load_skill" => {
            let name = tc.arguments.get("skill_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match ctx.skill_index.get(name) {
                Some(skill) => {
                    debug!(
                        node = %ctx.node_id,
                        skill = %name,
                        content_len = skill.content.len(),
                        "load_skill: returning skill content"
                    );
                    (skill.content.clone(), None)
                }
                None => {
                    warn!(node = %ctx.node_id, skill = %name, "load_skill: unknown skill");
                    (format!("Unknown skill: {}", name), None)
                }
            }
        }
        "load_skill_reference" => {
            let path = tc.arguments.get("reference_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let valid = ctx.skill_index.values()
                .any(|s| s.references.contains(&path.to_string()));
            if valid {
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        debug!(
                            node = %ctx.node_id,
                            path = %path,
                            content_len = content.len(),
                            "load_skill_reference: returning file content"
                        );
                        (content, None)
                    }
                    Err(e) => {
                        warn!(node = %ctx.node_id, path = %path, error = %e, "load_skill_reference: read failed");
                        (format!("Failed to read reference '{}': {}", path, e), None)
                    }
                }
            } else {
                warn!(node = %ctx.node_id, path = %path, "load_skill_reference: path not declared");
                (format!("Reference path not declared in any skill: {}", path), None)
            }
        }
        _ => {
            // Check if this is an MCP tool
            if let Some(mcp_tool) = ctx.mcp_tool_index.get(tc.name.as_str()) {
                if let Some(server_def) = ctx.config.mcp_servers.get(&mcp_tool.server_id) {
                    debug!(
                        node = %ctx.node_id,
                        tool = %tc.name,
                        server = %mcp_tool.server_id,
                        "Dispatching MCP tool call"
                    );
                    let mut client = super::mcp::McpClient::new_pub(
                        &server_def.url, server_def.auth.as_ref()
                    );
                    match client.initialize().await {
                        Ok(_) => {
                            match client.call_tool(&tc.name, tc.arguments.clone()).await {
                                Ok(result) => {
                                    let content = serde_json::to_string(&result).unwrap_or_default();
                                    return (content, None);
                                }
                                Err(e) => return (format!("MCP tool error: {}", e), None),
                            }
                        }
                        Err(e) => return (format!("MCP initialize error: {}", e), None),
                    }
                } else {
                    return (format!("MCP server '{}' not found", mcp_tool.server_id), None);
                }
            }

            // Subgraph tool dispatch (existing logic)
            let tool_ref = ctx.node_config.tools.iter()
                .find(|t| t.name == tc.name);

            if let Some(tool) = tool_ref {
                // Parse arguments as input HashMap
                let input: HashMap<String, Value> = match &tc.arguments {
                    Value::Object(obj) => obj.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    other => {
                        warn!(
                            node = %ctx.node_id,
                            tool = %tc.name,
                            arguments = ?other,
                            "Tool call arguments is not an object, using empty input"
                        );
                        HashMap::new()
                    }
                };

                // Execute subgraph
                if let Some(subgraph) = ctx.config.subgraphs.get(&tool.subgraph_ref) {
                    match super::subgraph_runner::execute(subgraph, input, ctx.config).await {
                        Ok(result) => {
                            let usage = crate::providers::TokenUsage {
                                input_tokens: result.usage.input_tokens,
                                output_tokens: result.usage.output_tokens,
                                total_tokens: result.usage.input_tokens + result.usage.output_tokens,
                                thinking_tokens: None,
                                search_count: None,
                            };
                            let content = serde_json::to_string(&result.outputs).unwrap_or_default();
                            (content, Some(usage))
                        }
                        Err(e) => (format!("Tool execution error: {}", e), None),
                    }
                } else {
                    (format!("Subgraph '{}' not found", tool.subgraph_ref), None)
                }
            } else {
                (format!("Unknown tool: {}", tc.name), None)
            }
        }
    }
}

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

        // Build skill index from upstream skill_set nodes
        let skill_index = build_skill_index(&node_config.skill_sets, state, node_id)?;

        // Build MCP tool index from upstream mcp_tools nodes
        let mcp_tool_index = build_mcp_tool_index(&node_config.mcp_tool_sets, state, node_id)?;

        // Build skill metadata for system_prompt injection
        let skill_metadata = if !skill_index.is_empty() {
            let lines: Vec<String> = skill_index.iter()
                .map(|(name, skill)| format!("- **{}**: {}", name, skill.description))
                .collect();
            Some(format!(
                "## Available Skills\nUse the `load_skill` tool to load full instructions for any skill.\n{}",
                lines.join("\n")
            ))
        } else {
            None
        };

        // Build prompt
        let (user_prompt, system_prompt, output_schema) = if let Some(tpl) = template {
            let user = render_template(&tpl.user_prompt, &variables);
            let template_system = tpl.system_prompt.as_ref()
                .map(|s| render_template(s, &variables));

            // Compose system_prompt: harness instructions + skill metadata + template system_prompt
            let system = compose_system_prompt(
                config.harness_instructions.as_deref(),
                skill_metadata.as_deref(),
                template_system.as_deref(),
            );

            (user, system, Some(&tpl.output_schema))
        } else {
            // No template - use variables as prompt
            let prompt = variables.iter()
                .map(|(k, v)| format!("{}: {}", k, value_to_string(v)))
                .collect::<Vec<_>>()
                .join("\n");
            // Still inject harness + skills even without template
            let system = compose_system_prompt(
                config.harness_instructions.as_deref(),
                skill_metadata.as_deref(),
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

        // Tool definitions: user-defined subgraph tools + built-in skill tools
        let mut all_tool_defs: Vec<crate::providers::ToolDefinition> = Vec::new();

        // 1. User-defined subgraph tools
        all_tool_defs.extend(node_config.tools.iter().map(|t| crate::providers::ToolDefinition {
            name: t.name.clone(),
            description: t.description.clone().unwrap_or_default(),
            parameters: t.parameters.clone().unwrap_or(json!({
                "type": "object",
                "properties": {},
            })),
        }));

        // 2. Built-in skill tools (if skill_sets present and resolved)
        if !skill_index.is_empty() {
            // Naming conflict check
            let reserved = ["load_skill", "load_skill_reference"];
            for t in &node_config.tools {
                if reserved.contains(&t.name.as_str()) {
                    return Err(EngineError::NodeExecutionFailed(
                        format!("Node '{}': tool name '{}' conflicts with built-in skill tool", node_id, t.name)
                    ));
                }
            }

            // load_skill: enum of skill names
            let names: Vec<String> = skill_index.keys().cloned().collect();
            all_tool_defs.push(crate::providers::ToolDefinition {
                name: "load_skill".into(),
                description: format!("Load detailed instructions for a skill. Available: {}", names.join(", ")),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "skill_name": { "type": "string", "enum": names }
                    },
                    "required": ["skill_name"]
                }),
            });

            // load_skill_reference: only if any skills have references
            let all_refs: Vec<&String> = skill_index.values()
                .flat_map(|s| &s.references)
                .collect();
            if !all_refs.is_empty() {
                all_tool_defs.push(crate::providers::ToolDefinition {
                    name: "load_skill_reference".into(),
                    description: format!(
                        "Load a reference file from a skill. Available: {}",
                        all_refs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "reference_path": { "type": "string" }
                        },
                        "required": ["reference_path"]
                    }),
                });
            }
        }

        // 3. MCP tools from mcp_tools nodes
        for (tool_name, mcp_tool) in &mcp_tool_index {
            // Naming conflict check against user tools and skill tools
            if node_config.tools.iter().any(|t| t.name == *tool_name) {
                return Err(EngineError::NodeExecutionFailed(
                    format!("Node '{}': MCP tool '{}' conflicts with user-defined tool", node_id, tool_name)
                ));
            }
            if skill_index.contains_key(tool_name) || tool_name == "load_skill" || tool_name == "load_skill_reference" {
                return Err(EngineError::NodeExecutionFailed(
                    format!("Node '{}': MCP tool '{}' conflicts with built-in tool", node_id, tool_name)
                ));
            }

            all_tool_defs.push(crate::providers::ToolDefinition {
                name: tool_name.clone(),
                description: mcp_tool.description.clone(),
                parameters: mcp_tool.input_schema.clone(),
            });
        }

        // Register all tools if model supports function calling
        if !all_tool_defs.is_empty() {
            let supports_fc = model_caps.map(|c| c.function_calling).unwrap_or(true);
            if supports_fc {
                options = options.with_tools(all_tool_defs);
            } else {
                warn!(
                    node = %node_id,
                    provider = %provider_id,
                    model = %model_id,
                    "Model doesn't support function calling, skill and user tools unavailable"
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

            let dispatch_ctx = ToolDispatchContext {
                node_config,
                config,
                skill_index: &skill_index,
                mcp_tool_index: &mcp_tool_index,
                node_id,
            };

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

                        // Dispatch each tool call (built-in skills or subgraph)
                        for tc in tool_calls {
                            let (result_content, sub_usage) = dispatch_tool_call(tc, &dispatch_ctx).await;
                            if let Some(usage) = sub_usage {
                                total_usage.input_tokens += usage.input_tokens;
                                total_usage.output_tokens += usage.output_tokens;
                                total_usage.total_tokens += usage.total_tokens;
                            }
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

/// Build a skill index from upstream skill_set node outputs in state.
/// Returns a HashMap keyed by skill name, containing resolved content and references.
fn build_skill_index(
    skill_set_ids: &[String],
    state: &ExecutionState,
    node_id: &str,
) -> Result<HashMap<String, ResolvedSkill>, EngineError> {
    let mut index = HashMap::new();

    for ss_id in skill_set_ids {
        let skills_path = format!("{}.skills", ss_id);
        let skills_value = state.read(&skills_path)
            .ok_or_else(|| {
                error!(
                    node = %node_id,
                    skill_set = %ss_id,
                    "Skill set node output not found in state — ensure edge from skill_set to generate"
                );
                EngineError::NodeExecutionFailed(
                    format!("Node '{}': skill set '{}' not found in state", node_id, ss_id)
                )
            })?;

        let skills_array = skills_value.as_array()
            .ok_or_else(|| EngineError::NodeExecutionFailed(
                format!("Node '{}': skill set '{}' output is not an array", node_id, ss_id)
            ))?;

        for skill_val in skills_array {
            let name = skill_val.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() { continue; }

            if index.contains_key(&name) {
                warn!(
                    node = %node_id,
                    skill = %name,
                    skill_set = %ss_id,
                    "Duplicate skill name, overwriting previous"
                );
            }

            let description = skill_val.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let content = skill_val.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let references = skill_val.get("references")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect())
                .unwrap_or_default();

            index.insert(name, ResolvedSkill { description, content, references });
        }
    }

    Ok(index)
}

/// Build MCP tool index from upstream mcp_tools node outputs in state.
/// Returns a HashMap keyed by tool name, containing description, inputSchema, and server_id.
fn build_mcp_tool_index(
    mcp_tool_set_ids: &[String],
    state: &ExecutionState,
    node_id: &str,
) -> Result<HashMap<String, ResolvedMcpTool>, EngineError> {
    let mut index = HashMap::new();

    for mt_id in mcp_tool_set_ids {
        let tools_path = format!("{}.tools", mt_id);
        let tools_value = state.read(&tools_path)
            .ok_or_else(|| {
                error!(node = %node_id, mcp_tools = %mt_id, "MCP tools node output not found in state");
                EngineError::NodeExecutionFailed(
                    format!("Node '{}': mcp_tools '{}' not found in state", node_id, mt_id)
                )
            })?;

        let server_path = format!("{}.server", mt_id);
        let server_id = state.read(&server_path)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| mt_id.clone());

        let tools_array = tools_value.as_array()
            .ok_or_else(|| EngineError::NodeExecutionFailed(
                format!("Node '{}': mcp_tools '{}' output is not an array", node_id, mt_id)
            ))?;

        for tool_val in tools_array {
            let name = tool_val.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() { continue; }

            let description = tool_val.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let input_schema = tool_val.get("inputSchema")
                .cloned()
                .unwrap_or(json!({
                    "type": "object",
                    "properties": {},
                }));

            if index.contains_key(&name) {
                warn!(node = %node_id, tool = %name, "Duplicate MCP tool name, overwriting");
            }

            index.insert(name, ResolvedMcpTool {
                description,
                input_schema,
                server_id: server_id.clone(),
            });
        }
    }

    Ok(index)
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
