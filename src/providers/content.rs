//! Content formatting utilities for different provider APIs
//!
//! Each provider has different formats for multimodal content (text + images).
//! This module provides format functions for each provider type.

use serde_json::{json, Value};
use tracing::warn;
use super::{MessageContent, ContentPart, Message, MessageRole};

/// Format content for OpenAI-compatible APIs
/// Used by: OpenAI, DeepSeek, xAI, Qwen, Zhipu, Moonshot
///
/// Text: returns string directly
/// Parts: returns array with image_url format
pub fn format_openai_content(content: &MessageContent) -> Value {
    match content {
        MessageContent::Text(text) => json!(text),
        MessageContent::Parts(parts) => {
            let formatted: Vec<Value> = parts.iter().map(|part| {
                match part {
                    ContentPart::Text { text } => json!({
                        "type": "text",
                        "text": text
                    }),
                    ContentPart::Image { media_type, data } => json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{};base64,{}", media_type, data)
                        }
                    }),
                    ContentPart::ImageUrl { url } => json!({
                        "type": "image_url",
                        "image_url": {
                            "url": url
                        }
                    }),
                }
            }).collect();
            json!(formatted)
        }
    }
}

/// Format content for Anthropic API
/// Image format uses source.type = "base64"
///
/// Text: returns string directly
/// Parts: returns array with Anthropic image format
pub fn format_anthropic_content(content: &MessageContent) -> Value {
    match content {
        MessageContent::Text(text) => json!(text),
        MessageContent::Parts(parts) => {
            let formatted: Vec<Value> = parts.iter().map(|part| {
                match part {
                    ContentPart::Text { text } => json!({
                        "type": "text",
                        "text": text
                    }),
                    ContentPart::Image { media_type, data } => json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data
                        }
                    }),
                    ContentPart::ImageUrl { url } => json!({
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": url
                        }
                    }),
                }
            }).collect();
            json!(formatted)
        }
    }
}

/// Format content for Gemini API (always returns array of parts)
/// Uses inline_data for images
///
/// Text: returns [{ "text": ... }]
/// Parts: returns array with inline_data format
pub fn format_gemini_parts(content: &MessageContent) -> Value {
    match content {
        MessageContent::Text(text) => json!([{ "text": text }]),
        MessageContent::Parts(parts) => {
            let formatted: Vec<Value> = parts.iter().map(|part| {
                match part {
                    ContentPart::Text { text } => json!({ "text": text }),
                    ContentPart::Image { media_type, data } => json!({
                        "inline_data": {
                            "mime_type": media_type,
                            "data": data
                        }
                    }),
                    ContentPart::ImageUrl { url } => {
                        warn!(url = %url, "Gemini does not support HTTP image URLs, skipping image");
                        json!({ "text": "[Image URL not supported by Gemini]" })
                    },
                }
            }).collect();
            json!(formatted)
        }
    }
}

/// Build OpenAI-format messages array from messages and options
/// System prompt is included as first message if provided
pub fn build_openai_messages(messages: &[Message], system_prompt: Option<&str>) -> Vec<Value> {
    let mut result = Vec::new();

    // Add system prompt first if provided
    if let Some(prompt) = system_prompt {
        result.push(json!({
            "role": "system",
            "content": prompt
        }));
    }

    // Add all messages
    for msg in messages {
        let role = match msg.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let mut msg_json = json!({
            "role": role,
            "content": format_openai_content(&msg.content)
        });
        // Tool result messages need tool_call_id
        if let Some(ref id) = msg.tool_call_id {
            msg_json["tool_call_id"] = json!(id);
        }
        // Assistant messages with tool_calls
        if let Some(ref calls) = msg.tool_calls {
            msg_json["tool_calls"] = json!(calls.iter().map(|tc| json!({
                "id": tc.id,
                "type": "function",
                "function": {
                    "name": tc.name,
                    "arguments": tc.arguments.to_string(),
                }
            })).collect::<Vec<_>>());
        }
        result.push(msg_json);
    }

    result
}

/// Build Anthropic-format messages array (excludes system - handled separately)
pub fn build_anthropic_messages(messages: &[Message]) -> Vec<Value> {
    messages.iter()
        .filter(|msg| msg.role != MessageRole::System)
        .map(|msg| {
            match msg.role {
                MessageRole::Tool => {
                    // Anthropic tool results: role=user, content=[{type: tool_result, ...}]
                    json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": msg.tool_call_id.as_deref().unwrap_or(""),
                            "content": match &msg.content {
                                MessageContent::Text(text) => text.clone(),
                                MessageContent::Parts(_) => "".to_string(),
                            }
                        }]
                    })
                }
                MessageRole::Assistant if msg.tool_calls.is_some() => {
                    // Assistant message with tool_use blocks
                    let mut content_blocks: Vec<Value> = Vec::new();
                    // Add text block if there's content
                    let text = match &msg.content {
                        MessageContent::Text(t) => t.clone(),
                        _ => String::new(),
                    };
                    if !text.is_empty() {
                        content_blocks.push(json!({"type": "text", "text": text}));
                    }
                    // Add tool_use blocks
                    if let Some(ref calls) = msg.tool_calls {
                        for tc in calls {
                            content_blocks.push(json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments,
                            }));
                        }
                    }
                    json!({
                        "role": "assistant",
                        "content": content_blocks
                    })
                }
                _ => {
                    let role = match msg.role {
                        MessageRole::User => "user",
                        MessageRole::Assistant => "assistant",
                        MessageRole::System => unreachable!(),
                        MessageRole::Tool => unreachable!(), // handled above
                    };
                    json!({
                        "role": role,
                        "content": format_anthropic_content(&msg.content)
                    })
                }
            }
        })
        .collect()
}

/// Build Gemini-format contents array
/// Gemini uses "model" instead of "assistant" for role
pub fn build_gemini_contents(messages: &[Message]) -> Vec<Value> {
    messages.iter()
        .filter(|msg| msg.role != MessageRole::System) // system handled separately
        .map(|msg| {
            match msg.role {
                MessageRole::Tool => {
                    // Gemini tool results: role=function, functionResponse format
                    let content_text = match &msg.content {
                        MessageContent::Text(t) => t.clone(),
                        _ => String::new(),
                    };
                    let response_value = serde_json::from_str::<Value>(&content_text)
                        .unwrap_or_else(|_| json!({"result": content_text}));
                    // Use tool_name (preferred) or fallback to tool_call_id
                    let fn_name = msg.tool_name.as_deref()
                        .or(msg.tool_call_id.as_deref())
                        .unwrap_or("tool");
                    json!({
                        "role": "function",
                        "parts": [{
                            "functionResponse": {
                                "name": fn_name,
                                "response": response_value
                            }
                        }]
                    })
                }
                MessageRole::Assistant if msg.tool_calls.is_some() => {
                    // Assistant message with functionCall parts
                    let mut parts: Vec<Value> = Vec::new();
                    let text = match &msg.content {
                        MessageContent::Text(t) => t.clone(),
                        _ => String::new(),
                    };
                    if !text.is_empty() {
                        parts.push(json!({"text": text}));
                    }
                    if let Some(ref calls) = msg.tool_calls {
                        for tc in calls {
                            parts.push(json!({
                                "functionCall": {
                                    "name": tc.name,
                                    "args": tc.arguments,
                                }
                            }));
                        }
                    }
                    json!({
                        "role": "model",
                        "parts": parts
                    })
                }
                _ => {
                    let role = match msg.role {
                        MessageRole::User => "user",
                        MessageRole::Assistant => "model",
                        MessageRole::System => unreachable!(),
                        MessageRole::Tool => unreachable!(),
                    };
                    json!({
                        "role": role,
                        "parts": format_gemini_parts(&msg.content)
                    })
                }
            }
        })
        .collect()
}

/// Extract system prompt from messages (for Anthropic/Gemini which handle it separately)
pub fn extract_system_prompt(messages: &[Message]) -> Option<String> {
    messages.iter()
        .find(|msg| msg.role == MessageRole::System)
        .and_then(|msg| {
            match &msg.content {
                MessageContent::Text(text) => Some(text.clone()),
                MessageContent::Parts(parts) => {
                    // Concatenate all text parts
                    let text: String = parts.iter()
                        .filter_map(|p| match p {
                            ContentPart::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() { None } else { Some(text) }
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_openai_content_text() {
        let content = MessageContent::Text("Hello".to_string());
        let formatted = format_openai_content(&content);
        assert_eq!(formatted, json!("Hello"));
    }

    #[test]
    fn test_format_openai_content_parts() {
        let content = MessageContent::Parts(vec![
            ContentPart::Text { text: "What's in this image?".to_string() },
            ContentPart::Image {
                media_type: "image/png".to_string(),
                data: "base64data".to_string(),
            },
        ]);
        let formatted = format_openai_content(&content);

        assert!(formatted.is_array());
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "image_url");
        assert_eq!(arr[1]["image_url"]["url"], "data:image/png;base64,base64data");
    }

    #[test]
    fn test_format_anthropic_content_image() {
        let content = MessageContent::Parts(vec![
            ContentPart::Image {
                media_type: "image/jpeg".to_string(),
                data: "imgdata".to_string(),
            },
        ]);
        let formatted = format_anthropic_content(&content);

        let arr = formatted.as_array().unwrap();
        assert_eq!(arr[0]["type"], "image");
        assert_eq!(arr[0]["source"]["type"], "base64");
        assert_eq!(arr[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(arr[0]["source"]["data"], "imgdata");
    }

    #[test]
    fn test_format_gemini_parts_text() {
        let content = MessageContent::Text("Hello".to_string());
        let formatted = format_gemini_parts(&content);

        assert!(formatted.is_array());
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["text"], "Hello");
    }

    #[test]
    fn test_format_gemini_parts_image() {
        let content = MessageContent::Parts(vec![
            ContentPart::Image {
                media_type: "image/png".to_string(),
                data: "pngdata".to_string(),
            },
        ]);
        let formatted = format_gemini_parts(&content);

        let arr = formatted.as_array().unwrap();
        assert_eq!(arr[0]["inline_data"]["mime_type"], "image/png");
        assert_eq!(arr[0]["inline_data"]["data"], "pngdata");
    }

    #[test]
    fn test_build_openai_messages() {
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi there!"),
        ];
        let result = build_openai_messages(&messages, Some("You are helpful"));

        assert_eq!(result.len(), 3);
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[0]["content"], "You are helpful");
        assert_eq!(result[1]["role"], "user");
        assert_eq!(result[2]["role"], "assistant");
    }

    #[test]
    fn test_format_openai_content_image_url() {
        let content = MessageContent::Parts(vec![
            ContentPart::Text { text: "Describe this".to_string() },
            ContentPart::ImageUrl { url: "https://example.com/photo.jpg".to_string() },
        ]);
        let formatted = format_openai_content(&content);

        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["type"], "image_url");
        assert_eq!(arr[1]["image_url"]["url"], "https://example.com/photo.jpg");
    }

    #[test]
    fn test_format_anthropic_content_image_url() {
        let content = MessageContent::Parts(vec![
            ContentPart::ImageUrl { url: "https://example.com/photo.jpg".to_string() },
        ]);
        let formatted = format_anthropic_content(&content);

        let arr = formatted.as_array().unwrap();
        assert_eq!(arr[0]["type"], "image");
        assert_eq!(arr[0]["source"]["type"], "url");
        assert_eq!(arr[0]["source"]["url"], "https://example.com/photo.jpg");
    }

    #[test]
    fn test_format_gemini_parts_image_url_fallback() {
        let content = MessageContent::Parts(vec![
            ContentPart::ImageUrl { url: "https://example.com/photo.jpg".to_string() },
        ]);
        let formatted = format_gemini_parts(&content);

        let arr = formatted.as_array().unwrap();
        // Gemini doesn't support HTTP URLs, should fall back to text placeholder
        assert!(arr[0]["text"].as_str().unwrap().contains("not supported"));
    }

    #[test]
    fn test_build_gemini_contents() {
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi!"),
        ];
        let result = build_gemini_contents(&messages);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "user");
        assert_eq!(result[1]["role"], "model"); // Gemini uses "model"
    }
}
