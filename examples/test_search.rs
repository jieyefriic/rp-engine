//! Test enable_search with Qwen provider
//!
//! Run with: cargo run --example test_search

use riceprompt_engine::{QwenProvider, LLMProvider, Message, ChatOptions};

const QWEN_KEY: &str = "REDACTED_DASHSCOPE_KEY";

#[tokio::main]
async fn main() {
    let qwen = QwenProvider::new(QWEN_KEY);

    // Test 1: With search enabled (should return real-time info)
    println!("=== Test 1: enable_search = true ===");
    let messages = vec![Message::user("今天杭州天气怎么样？")];
    let options = ChatOptions::new()
        .with_model("qwen-plus")
        .with_temperature(0.7)
        .with_max_tokens(300)
        .with_enable_search(true);

    match qwen.chat(messages, options).await {
        Ok(resp) => {
            println!("Model: {}", resp.model);
            println!("Content: {}", resp.content.trim());
            println!("Tokens: {} in, {} out, search_count: {:?}",
                resp.usage.input_tokens, resp.usage.output_tokens, resp.usage.search_count);
        }
        Err(e) => println!("FAILED: {:?}", e),
    }

    println!();

    // Test 2: Without search (should not have real-time info)
    println!("=== Test 2: enable_search = false (default) ===");
    let messages = vec![Message::user("今天杭州天气怎么样？")];
    let options = ChatOptions::new()
        .with_model("qwen-plus")
        .with_temperature(0.7)
        .with_max_tokens(300);

    match qwen.chat(messages, options).await {
        Ok(resp) => {
            println!("Model: {}", resp.model);
            println!("Content: {}", resp.content.trim());
            println!("Tokens: {} in, {} out", resp.usage.input_tokens, resp.usage.output_tokens);
        }
        Err(e) => println!("FAILED: {:?}", e),
    }
}
