//! Test tool calling with Anthropic and Gemini
//!
//! Run with: cargo run --example test_tool_calling

use riceprompt_engine::Engine;
use serde_json::json;

// Set via environment variables:
//   ANTHROPIC_API_KEY=sk-ant-...
//   GEMINI_API_KEY=AIza...
fn get_key(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("Missing env var: {}. Set it before running.", name);
        std::process::exit(1);
    })
}

fn tool_calling_yaml(provider: &str, model: &str) -> String {
    let yaml = format!(
r#"version: "1.0"
name: "test_tool_calling_{provider}"
description: "Test tool calling with {provider}"

nodes:
  - id: start
    type: start
    config:
      output:
        query: "input.query"

  - id: agent
    type: generate
    config:
      provider: {provider}
      model: {model}
      template: agent_prompt
      tools:
        - name: get_weather
          description: "Get the current weather for a city. Returns a short weather report."
          ref: sg_weather
          parameters:
            type: object
            properties:
              city:
                type: string
                description: "The city name, e.g. Tokyo, London, Beijing"
            required:
              - city
      max_tool_rounds: 3
      variables:
        question: "start.query"

  - id: response
    type: response
    config:
      output:
        answer: "agent.output"

edges:
  - from: start
    to: agent
  - from: agent
    to: response

templates:
  agent_prompt:
    system_prompt: "You are a helpful assistant. When asked about weather, you MUST use the get_weather tool. Do not make up weather data."
    user_prompt: "{{{{question}}}}"
    config:
      temperature: 0.3
      max_tokens: 500

  mock_weather_tpl:
    user_prompt: "Return ONLY this text, nothing else: Sunny, 25C, humidity 60% in {{{{city}}}}"
    config:
      temperature: 0
      max_tokens: 50

subgraphs:
  sg_weather:
    nodes:
      - id: start
        type: start
      - id: mock_api
        type: generate
        config:
          provider: {provider}
          model: {model}
          template: mock_weather_tpl
          variables:
            city: "start.city"
      - id: response
        type: response
        config:
          output:
            result: "mock_api.output"
    edges:
      - from: start
        to: mock_api
      - from: mock_api
        to: response
"#,
        provider = provider,
        model = model,
    );
    yaml
}

async fn run_test(provider_name: &str, provider_id: &str, model: &str, api_key: &str) {
    println!("\n{}", "=".repeat(60));
    println!("TEST: {} - Tool Calling", provider_name);
    println!("{}", "=".repeat(60));

    let engine = Engine::builder()
        .provider(provider_id, api_key)
        .build()
        .await
        .unwrap();

    let yaml = tool_calling_yaml(provider_id, model);
    let input = json!({
        "query": "What's the weather like in Tokyo?"
    });

    match engine.run_yaml(&yaml, input).await {
        Ok(result) => {
            println!("\n\u{2705} {} tool calling SUCCESS", provider_name);
            println!("   Status: {:?}", result.status);
            println!("   Tokens: {} in / {} out",
                result.token_usage.total_input_tokens,
                result.token_usage.total_output_tokens);
            for (node_id, records) in &result.executions {
                if let Some(last) = records.last() {
                    println!("   [{}]:", node_id);
                    for (k, v) in &last.outputs {
                        if k.starts_with('_') { continue; } // skip internal fields
                        let val = v.to_string();
                        let preview: String = val.chars().take(200).collect();
                        println!("     {} = {}", k, preview);
                    }
                }
            }
        }
        Err(e) => {
            println!("\n\u{274C} {} tool calling FAILED: {}", provider_name, e);
        }
    }
}

#[tokio::main]
async fn main() {
    println!("Tool Calling Integration Test\n");

    let anthropic_key = get_key("ANTHROPIC_API_KEY");
    let gemini_key = get_key("GEMINI_API_KEY");

    run_test("Anthropic Claude", "anthropic", "claude-sonnet-4-20250514", &anthropic_key).await;
    run_test("Google Gemini", "gemini", "gemini-2.0-flash", &gemini_key).await;

    println!("\n{}", "=".repeat(60));
    println!("DONE");
    println!("{}", "=".repeat(60));
}
