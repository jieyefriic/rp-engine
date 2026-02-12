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
            let system = tpl.system_prompt.as_ref()
                .map(|s| render_template(s, &variables));
            (user, system, Some(&tpl.output_schema))
        } else {
            // No template - use variables as prompt
            let prompt = variables.iter()
                .map(|(k, v)| format!("{}: {}", k, value_to_string(v)))
                .collect::<Vec<_>>()
                .join("\n");
            (if prompt.is_empty() { "Hello".into() } else { prompt }, None, None)
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

        // Check for images in variables (multimodal support)
        let mut image_parts = Vec::new();
        for (var_name, value) in &variables {
            // Variables ending with _image or _image_base64 are treated as images
            if var_name.ends_with("_image") || var_name.ends_with("_image_base64") {
                if let Value::String(data) = value {
                    if !data.is_empty() {
                        // Determine if it's a URL or base64
                        let (media_type, image_data) = if data.starts_with("data:") {
                            // Data URI format: data:image/jpeg;base64,xxx
                            if let Some(comma_pos) = data.find(',') {
                                let prefix = &data[..comma_pos];
                                let base64_data = &data[comma_pos + 1..];

                                // Extract media type from data URI
                                let media = if let Some(semicolon) = prefix.find(';') {
                                    prefix[5..semicolon].to_string() // Skip "data:"
                                } else {
                                    "image/jpeg".to_string()
                                };

                                (media, base64_data.to_string())
                            } else {
                                ("image/jpeg".to_string(), data.clone())
                            }
                        } else if data.starts_with("http://") || data.starts_with("https://") {
                            // URL format - providers will handle it
                            ("image/jpeg".to_string(), data.clone())
                        } else {
                            // Pure base64 string, assume JPEG
                            ("image/jpeg".to_string(), data.clone())
                        };

                        image_parts.push(ContentPart::Image {
                            media_type,
                            data: image_data,
                        });
                    }
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
            }
        };

        messages.push(user_message);

        // Build chat options
        let mut options = ChatOptions::new()
            .with_model(model_id);

        // Apply template config
        if let Some(tpl) = template {
            options = options
                .with_temperature(tpl.config.temperature)
                .with_max_tokens(tpl.config.max_tokens);

            if tpl.config.enable_search {
                options = options.with_enable_search(true);
            }
        }

        // Enable JSON mode if we have output schema
        if output_schema.map(|s| !s.is_empty()).unwrap_or(false) {
            options = options.with_json_output();
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
        // 3) no JSON schema (JSON mode needs complete response)
        let has_json_schema = output_schema.map(|s| !s.is_empty()).unwrap_or(false);
        let should_stream = node_config.stream
            && config.stream_handler.is_some()
            && !has_json_schema;

        // Make the API call (streaming or non-streaming)
        // Returns: (content, thinking, usage, ttft)
        let (content, thinking, usage, ttft) = if should_stream {
            // Streaming path: wrap StreamHandler into StreamCallback, call provider.stream()
            let handler = config.stream_handler.as_ref().unwrap().clone();
            let node_id_owned = node_id.clone();
            let callback: StreamCallback = Box::new(move |chunk: &str, accumulated: &str| {
                handler.on_text_chunk(&node_id_owned, chunk, accumulated);
            });

            debug!(
                node = %node_id,
                provider = %provider_id,
                model = %model_id,
                template = template_name.as_deref().unwrap_or("(none)"),
                has_images = has_images,
                "Making streaming LLM API call"
            );

            match provider.stream(messages, options, callback).await {
                Ok(r) => {
                    debug!(
                        node = %node_id,
                        provider = %provider_id,
                        model = %model_id,
                        response_len = r.content.len(),
                        ttft_ms = ?r.ttft,
                        has_usage = r.usage.is_some(),
                        "Streaming LLM call completed"
                    );
                    (r.content, None, r.usage, r.ttft)
                }
                Err(e) => {
                    error!(
                        node = %node_id,
                        provider = %provider_id,
                        model = %model_id,
                        template = template_name.as_deref().unwrap_or("(none)"),
                        error = %e,
                        "Streaming LLM API call failed"
                    );
                    return Err(EngineError::ProviderError(e));
                }
            }
        } else {
            // Non-streaming path: call provider.chat()
            match provider.chat(messages, options).await {
                Ok(r) => {
                    debug!(
                        node = %node_id,
                        provider = %provider_id,
                        model = %model_id,
                        input_tokens = r.usage.input_tokens,
                        output_tokens = r.usage.output_tokens,
                        response_len = r.content.len(),
                        "LLM call completed successfully"
                    );
                    (r.content, r.thinking, Some(r.usage), None)
                }
                Err(e) => {
                    error!(
                        node = %node_id,
                        provider = %provider_id,
                        model = %model_id,
                        template = template_name.as_deref().unwrap_or("(none)"),
                        error = %e,
                        "LLM API call failed"
                    );
                    return Err(EngineError::ProviderError(e));
                }
            }
        };

        // Build outputs
        let mut outputs = HashMap::new();

        // Try to parse JSON if we have a schema
        if let Some(schema) = output_schema {
            if !schema.is_empty() {
                match serde_json::from_str::<Value>(&content) {
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

/// Generate example JSON from output schema
fn generate_example_json(schema: &[crate::schema::OutputField]) -> String {
    let mut obj = serde_json::Map::new();

    for field in schema {
        if field.name.is_empty() {
            continue;
        }

        let example = if let Some(ex) = &field.example {
            // Try to parse example as JSON, fall back to string
            serde_json::from_str(ex).unwrap_or(Value::String(ex.clone()))
        } else {
            get_example_for_type(&field.field_type)
        };

        obj.insert(field.name.clone(), example);
    }

    serde_json::to_string_pretty(&obj).unwrap_or_default()
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
            },
            OutputField {
                name: "confidence".to_string(),
                field_type: "number".to_string(),
                description: None,
                example: Some("0.95".to_string()),
            },
        ];

        let json = generate_example_json(&schema);
        assert!(json.contains("answer"));
        assert!(json.contains("confidence"));
    }
}
