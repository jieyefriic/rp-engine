//! SSE (Server-Sent Events) streaming utilities
//!
//! Provides utilities for parsing SSE streams from LLM APIs.

use futures::StreamExt;
use std::time::Instant;

use crate::error::ProviderError;
use super::{StreamResponse, StreamCallback, TokenUsage};

/// Result from parsing a single SSE chunk.
/// A chunk may contain text content, token usage, or both.
#[derive(Debug, Clone, Default)]
pub struct ChunkParseResult {
    /// Text content extracted from this chunk (if any)
    pub content: Option<String>,
    /// Token usage info extracted from this chunk (if any, typically in the final chunk)
    pub usage: Option<TokenUsage>,
}

impl ChunkParseResult {
    pub fn text(content: String) -> Self {
        Self { content: Some(content), usage: None }
    }

    pub fn usage(usage: TokenUsage) -> Self {
        Self { content: None, usage: Some(usage) }
    }

    pub fn empty() -> Self {
        Self { content: None, usage: None }
    }
}

/// Parse SSE stream and accumulate content
/// Returns StreamResponse with full content, time-to-first-token, and token usage
pub async fn process_sse_stream<F>(
    response: reqwest::Response,
    mut parse_chunk: F,
    on_chunk: &StreamCallback,
) -> Result<StreamResponse, ProviderError>
where
    F: FnMut(&str) -> ChunkParseResult,
{
    let start_time = Instant::now();
    let mut first_token_time: Option<Instant> = None;
    let mut full_content = String::new();
    let mut buffer = String::new();
    let mut last_usage: Option<TokenUsage> = None;

    let mut stream = response.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        // Process complete lines
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            // SSE format: "data: {...}"
            if let Some(data) = line.strip_prefix("data: ") {
                // Skip [DONE] marker
                if data == "[DONE]" {
                    continue;
                }

                // Parse and extract content + usage
                let result = parse_chunk(data);

                if let Some(content) = result.content {
                    if !content.is_empty() {
                        // Record first token time
                        if first_token_time.is_none() {
                            first_token_time = Some(Instant::now());
                        }

                        full_content.push_str(&content);
                        on_chunk(&content, &full_content);
                    }
                }

                if let Some(usage) = result.usage {
                    last_usage = Some(usage);
                }
            }
        }
    }

    // Calculate TTFT in milliseconds
    let ttft = first_token_time.map(|t| {
        t.duration_since(start_time).as_secs_f64() * 1000.0
    });

    Ok(StreamResponse {
        content: full_content,
        ttft,
        usage: last_usage,
    })
}

/// Parse OpenAI-format SSE chunk
/// Extracts content from choices[0].delta.content and usage from the final chunk
pub fn parse_openai_chunk(data: &str) -> ChunkParseResult {
    let parsed: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return ChunkParseResult::empty(),
    };

    let content = parsed["choices"][0]["delta"]["content"]
        .as_str()
        .map(|s| s.to_string());

    let usage = parsed["usage"].as_object().map(|u| TokenUsage {
        input_tokens: u.get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: u.get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        total_tokens: u.get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        thinking_tokens: None,
        search_count: None,
    });

    ChunkParseResult { content, usage }
}

/// Parse Anthropic-format SSE chunk
/// Extracts content from content_block_delta events,
/// and usage from message_start (input_tokens) and message_delta (output_tokens)
pub fn parse_anthropic_chunk(data: &str) -> ChunkParseResult {
    let parsed: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return ChunkParseResult::empty(),
    };

    let event_type = match parsed["type"].as_str() {
        Some(t) => t,
        None => return ChunkParseResult::empty(),
    };

    match event_type {
        "content_block_delta" => {
            let content = parsed["delta"]["text"]
                .as_str()
                .map(|s| s.to_string());
            ChunkParseResult { content, usage: None }
        }
        "message_start" => {
            // message_start contains input_tokens in message.usage
            let usage = parsed["message"]["usage"].as_object().map(|u| {
                let input = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                TokenUsage {
                    input_tokens: input,
                    output_tokens: 0,
                    total_tokens: input,
                    thinking_tokens: None,
                    search_count: None,
                }
            });
            ChunkParseResult { content: None, usage }
        }
        "message_delta" => {
            // message_delta contains output_tokens in usage
            let usage = parsed["usage"].as_object().map(|u| {
                let output = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                TokenUsage {
                    input_tokens: 0,
                    output_tokens: output,
                    total_tokens: output,
                    thinking_tokens: None,
                    search_count: None,
                }
            });
            ChunkParseResult { content: None, usage }
        }
        _ => ChunkParseResult::empty(),
    }
}

/// Parse Gemini-format SSE chunk
/// Extracts content from parts (filtering out thought=true) and usage from usageMetadata
pub fn parse_gemini_chunk(data: &str) -> ChunkParseResult {
    let parsed: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return ChunkParseResult::empty(),
    };

    // Extract content from parts
    let content = parsed["candidates"][0]["content"]["parts"]
        .as_array()
        .map(|parts| {
            let text: String = parts.iter()
                .filter(|part| !part["thought"].as_bool().unwrap_or(false))
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            if text.is_empty() { None } else { Some(text) }
        })
        .flatten();

    // Extract usage from usageMetadata
    let usage = parsed["usageMetadata"].as_object().map(|meta| TokenUsage {
        input_tokens: meta.get("promptTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: meta.get("candidatesTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        total_tokens: meta.get("totalTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        thinking_tokens: meta.get("thoughtsTokenCount")
            .and_then(|v| v.as_u64()),
        search_count: None,
    });

    ChunkParseResult { content, usage }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_openai_chunk() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        let result = parse_openai_chunk(data);
        assert_eq!(result.content, Some("Hello".to_string()));
        assert!(result.usage.is_none());

        let data_empty = r#"{"choices":[{"delta":{}}]}"#;
        let result = parse_openai_chunk(data_empty);
        assert!(result.content.is_none());
    }

    #[test]
    fn test_parse_openai_chunk_with_usage() {
        let data = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#;
        let result = parse_openai_chunk(data);
        assert!(result.content.is_none());
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.total_tokens, 30);
    }

    #[test]
    fn test_parse_anthropic_chunk() {
        let data = r#"{"type":"content_block_delta","delta":{"text":"World"}}"#;
        let result = parse_anthropic_chunk(data);
        assert_eq!(result.content, Some("World".to_string()));

        let data_other = r#"{"type":"message_start","message":{"usage":{"input_tokens":15}}}"#;
        let result = parse_anthropic_chunk(data_other);
        assert!(result.content.is_none());
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 15);
    }

    #[test]
    fn test_parse_anthropic_message_delta() {
        let data = r#"{"type":"message_delta","usage":{"output_tokens":25}}"#;
        let result = parse_anthropic_chunk(data);
        assert!(result.content.is_none());
        let usage = result.usage.unwrap();
        assert_eq!(usage.output_tokens, 25);
    }

    #[test]
    fn test_parse_gemini_chunk() {
        // Normal content
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hi"}]}}]}"#;
        let result = parse_gemini_chunk(data);
        assert_eq!(result.content, Some("Hi".to_string()));

        // With thinking part (should be filtered)
        let data_thinking = r#"{"candidates":[{"content":{"parts":[{"text":"thinking...","thought":true},{"text":"answer"}]}}]}"#;
        let result = parse_gemini_chunk(data_thinking);
        assert_eq!(result.content, Some("answer".to_string()));
    }

    #[test]
    fn test_parse_gemini_chunk_with_usage() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"done"}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}}"#;
        let result = parse_gemini_chunk(data);
        assert_eq!(result.content, Some("done".to_string()));
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }
}
