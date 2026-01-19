//! Test all LLM providers with real API calls
//!
//! Run with: cargo run --example test_providers

use riceprompt_engine::{
    OpenAIProvider, AnthropicProvider, GeminiProvider,
    DeepSeekProvider, XAIProvider, QwenProvider, ZhipuProvider, MoonshotProvider,
    LLMProvider, Message, ChatOptions,
};

// API Keys
const OPENAI_KEY: &str = "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";
const ANTHROPIC_KEY: &str = "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";
const GEMINI_KEY: &str = "AIzaSyCryfzBwtxzr3yG-4NGyIatSivhSCurMDA";
const DEEPSEEK_KEY: &str = "sk-257ac7a0be094a42afe043bc2fcc682c";
const XAI_KEY: &str = "xai-wwEt0Lr8duEzoY3ClFvM0bpscdf0VE7IC0G8ZtdOoVveIGmFDOy76NIrKoIwMtz4bVLoV4ZztPgxSQBr";
const QWEN_KEY: &str = "sk-2a516181a8c6431888fe5312a0f23e46";
const ZHIPU_KEY: &str = "sk-2a516181a8c6431888fe5312a0f23e46";  // Same dashscope key
const MOONSHOT_KEY: &str = "sk-2a516181a8c6431888fe5312a0f23e46";  // Same dashscope key

async fn test_provider<P: LLMProvider>(provider: &P, name: &str) {
    println!("\n{}", "=".repeat(60));
    println!("Testing: {}", name);
    println!("{}", "=".repeat(60));

    let messages = vec![Message::user("Say 'Hello from Rust!' in exactly 5 words.")];
    let options = ChatOptions::new()
        .with_temperature(0.3)
        .with_max_tokens(50);

    match provider.chat(messages, options).await {
        Ok(response) => {
            println!("✅ SUCCESS");
            println!("   Model: {}", response.model);
            println!("   Content: {}", response.content.trim());
            println!("   Tokens: {} in, {} out",
                response.usage.input_tokens,
                response.usage.output_tokens
            );
        }
        Err(e) => {
            println!("❌ FAILED: {:?}", e);
        }
    }
}

#[tokio::main]
async fn main() {
    println!("🚀 RicePrompt-Engine Provider Tests");
    println!("Testing all providers with real API calls...\n");

    // OpenAI
    let openai = OpenAIProvider::new(OPENAI_KEY);
    test_provider(&openai, "OpenAI (gpt-4o-mini)").await;

    // Anthropic
    let anthropic = AnthropicProvider::new(ANTHROPIC_KEY);
    test_provider(&anthropic, "Anthropic (claude-sonnet-4)").await;

    // Gemini
    let gemini = GeminiProvider::new(GEMINI_KEY);
    test_provider(&gemini, "Gemini (gemini-2.0-flash)").await;

    // DeepSeek
    let deepseek = DeepSeekProvider::new(DEEPSEEK_KEY);
    test_provider(&deepseek, "DeepSeek (deepseek-chat)").await;

    // xAI
    let xai = XAIProvider::new(XAI_KEY);
    test_provider(&xai, "xAI (grok-2-latest)").await;

    // Qwen
    let qwen = QwenProvider::new(QWEN_KEY);
    test_provider(&qwen, "Qwen (qwen-turbo)").await;

    // Zhipu
    let zhipu = ZhipuProvider::new(ZHIPU_KEY);
    test_provider(&zhipu, "Zhipu (glm-4.7)").await;

    // Moonshot
    let moonshot = MoonshotProvider::new(MOONSHOT_KEY);
    test_provider(&moonshot, "Moonshot (kimi-k2-thinking)").await;

    println!("\n{}", "=".repeat(60));
    println!("All tests completed!");
    println!("{}", "=".repeat(60));
}
