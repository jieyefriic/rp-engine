//! Test all LLM providers with real API calls
//!
//! Run with: OPENAI_API_KEY=... ANTHROPIC_API_KEY=... cargo run --example test_providers
//!
//! Required env vars (set the ones you want to test):
//!   OPENAI_API_KEY, ANTHROPIC_API_KEY, GEMINI_API_KEY, DEEPSEEK_API_KEY,
//!   XAI_API_KEY, QWEN_API_KEY, ZHIPU_API_KEY, MOONSHOT_API_KEY

use riceprompt_engine::{
    OpenAIProvider, AnthropicProvider, GeminiProvider,
    DeepSeekProvider, XAIProvider, QwenProvider, ZhipuProvider, MoonshotProvider,
    LLMProvider, Message, ChatOptions,
};

fn get_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

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
    println!("RicePrompt-Engine Provider Tests");
    println!("Testing providers with API keys from environment variables...\n");

    let mut tested = 0;

    if let Some(key) = get_key("OPENAI_API_KEY") {
        let provider = OpenAIProvider::new(&key);
        test_provider(&provider, "OpenAI (gpt-4o-mini)").await;
        tested += 1;
    }

    if let Some(key) = get_key("ANTHROPIC_API_KEY") {
        let provider = AnthropicProvider::new(&key);
        test_provider(&provider, "Anthropic (claude-sonnet-4)").await;
        tested += 1;
    }

    if let Some(key) = get_key("GEMINI_API_KEY") {
        let provider = GeminiProvider::new(&key);
        test_provider(&provider, "Gemini (gemini-2.0-flash)").await;
        tested += 1;
    }

    if let Some(key) = get_key("DEEPSEEK_API_KEY") {
        let provider = DeepSeekProvider::new(&key);
        test_provider(&provider, "DeepSeek (deepseek-chat)").await;
        tested += 1;
    }

    if let Some(key) = get_key("XAI_API_KEY") {
        let provider = XAIProvider::new(&key);
        test_provider(&provider, "xAI (grok-2-latest)").await;
        tested += 1;
    }

    if let Some(key) = get_key("QWEN_API_KEY") {
        let provider = QwenProvider::new(&key);
        test_provider(&provider, "Qwen (qwen-turbo)").await;
        tested += 1;
    }

    if let Some(key) = get_key("ZHIPU_API_KEY") {
        let provider = ZhipuProvider::new(&key);
        test_provider(&provider, "Zhipu (glm-4.7)").await;
        tested += 1;
    }

    if let Some(key) = get_key("MOONSHOT_API_KEY") {
        let provider = MoonshotProvider::new(&key);
        test_provider(&provider, "Moonshot (kimi-k2-thinking)").await;
        tested += 1;
    }

    println!("\n{}", "=".repeat(60));
    if tested == 0 {
        println!("No API keys found. Set env vars to test providers.");
    } else {
        println!("Tested {} providers.", tested);
    }
    println!("{}", "=".repeat(60));
}
