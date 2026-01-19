//! SSE (Server-Sent Events) streaming utilities
//!
//! Provides utilities for parsing SSE streams from LLM APIs.

use futures::StreamExt;
use std::time::Instant;

use crate::error::ProviderError;
use super::{StreamResponse, StreamCallback};

/// Parse SSE stream and accumulate content
/// Returns StreamResponse with full content and time-to-first-token
pub async fn process_sse_stream<F>(
    response: reqwest::Response,
    mut parse_chunk: F,
    on_chunk: &StreamCallback,
) -> Result<StreamResponse, ProviderError>
where
    F: FnMut(&str) -> Option<String>,
{
    let start_time = Instant::now();
    let mut first_token_time: Option<Instant> = None;
    let mut full_content = String::new();
    let mut buffer = String::new();

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

                // Parse and extract content
                if let Some(content) = parse_chunk(data) {
                    if !content.is_empty() {
                        // Record first token time
                        if first_token_time.is_none() {
                            first_token_time = Some(Instant::now());
                        }

                        full_content.push_str(&content);
                        on_chunk(&content, &full_content);
                    }
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
    })
}

/// Parse OpenAI-format SSE chunk
/// Returns content from choices[0].delta.content
pub fn parse_openai_chunk(data: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(data).ok()?;
    parsed["choices"][0]["delta"]["content"]
        .as_str()
        .map(|s| s.to_string())
}

/// Parse Anthropic-format SSE chunk
/// Returns content from content_block_delta events
pub fn parse_anthropic_chunk(data: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(data).ok()?;

    // Anthropic uses event types
    let event_type = parsed["type"].as_str()?;

    if event_type == "content_block_delta" {
        parsed["delta"]["text"]
            .as_str()
            .map(|s| s.to_string())
    } else {
        None
    }
}

/// Parse Gemini-format SSE chunk
/// Returns content from parts, filtering out thought=true parts
pub fn parse_gemini_chunk(data: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(data).ok()?;

    let parts = parsed["candidates"][0]["content"]["parts"].as_array()?;

    // Filter out thinking parts (thought=true) and extract text
    let content: String = parts.iter()
        .filter(|part| !part["thought"].as_bool().unwrap_or(false))
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("");

    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_openai_chunk() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(parse_openai_chunk(data), Some("Hello".to_string()));

        let data_empty = r#"{"choices":[{"delta":{}}]}"#;
        assert_eq!(parse_openai_chunk(data_empty), None);
    }

    #[test]
    fn test_parse_anthropic_chunk() {
        let data = r#"{"type":"content_block_delta","delta":{"text":"World"}}"#;
        assert_eq!(parse_anthropic_chunk(data), Some("World".to_string()));

        let data_other = r#"{"type":"message_start"}"#;
        assert_eq!(parse_anthropic_chunk(data_other), None);
    }

    #[test]
    fn test_parse_gemini_chunk() {
        // Normal content
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hi"}]}}]}"#;
        assert_eq!(parse_gemini_chunk(data), Some("Hi".to_string()));

        // With thinking part (should be filtered)
        let data_thinking = r#"{"candidates":[{"content":{"parts":[{"text":"thinking...","thought":true},{"text":"answer"}]}}]}"#;
        assert_eq!(parse_gemini_chunk(data_thinking), Some("answer".to_string()));
    }
}
