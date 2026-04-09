//! Test harness instructions injection into generate node
//!
//! Run with: DEEPSEEK_API_KEY=sk-xxx cargo run --example test_harness

use riceprompt_engine::Engine;
use serde_json::json;

fn get_key(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("Missing env var: {}. Set it before running.", name);
        std::process::exit(1);
    })
}

const YAML_WITH_HARNESS: &str = r#"
version: "1.0"
name: "test_harness"
description: "Test harness instructions injection"

harness:
  instructions: |
    你是一个专业的翻译助手。
    规则：
    1. 始终用中文回复
    2. 翻译要自然流畅，不要直译
    3. 如果原文有专业术语，保留英文并加中文注释

nodes:
  - id: start
    type: start
    config:
      output:
        text: "input.text"

  - id: translate
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: translate_tpl
      variables:
        text: "start.text"

  - id: response
    type: response
    config:
      output:
        translation: "translate.output"

edges:
  - from: start
    to: translate
  - from: translate
    to: response

templates:
  translate_tpl:
    user_prompt: "Translate the following English text:\n\n{{text}}"
    config:
      temperature: 0.3
      max_tokens: 500
"#;

const YAML_WITHOUT_HARNESS: &str = r#"
version: "1.0"
name: "test_no_harness"
description: "Same workflow without harness - for comparison"

nodes:
  - id: start
    type: start
    config:
      output:
        text: "input.text"

  - id: translate
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: translate_tpl
      variables:
        text: "start.text"

  - id: response
    type: response
    config:
      output:
        translation: "translate.output"

edges:
  - from: start
    to: translate
  - from: translate
    to: response

templates:
  translate_tpl:
    system_prompt: "You are a translator."
    user_prompt: "Translate the following English text:\n\n{{text}}"
    config:
      temperature: 0.3
      max_tokens: 500
"#;

#[tokio::main]
async fn main() {
    let key = get_key("DEEPSEEK_API_KEY");
    let input = json!({ "text": "The harness engineering paradigm shifts focus from model intelligence to infrastructure reliability." });

    let engine = Engine::builder()
        .provider("deepseek", &key)
        .capture_actual_prompt(true)
        .build()
        .await
        .unwrap();

    // Test 1: With harness
    println!("{}", "=".repeat(60));
    println!("TEST 1: With Harness Instructions");
    println!("{}", "=".repeat(60));

    match engine.run_yaml(YAML_WITH_HARNESS, input.clone()).await {
        Ok(result) => {
            println!("\n\u{2705} SUCCESS");
            for (node_id, records) in &result.executions {
                if let Some(last) = records.last() {
                    if node_id == "translate" {
                        // Check system prompt contains harness
                        if let Some(sys) = last.outputs.get("_actual_system_prompt") {
                            println!("\n   System Prompt:\n   {}", sys);
                            if sys.to_string().contains("专业的翻译助手") {
                                println!("\n   \u{2705} Harness instructions FOUND in system_prompt");
                            } else {
                                println!("\n   \u{274C} Harness instructions NOT found in system_prompt");
                            }
                        }
                        if let Some(output) = last.outputs.get("output") {
                            println!("\n   Output: {}", output);
                        }
                    }
                }
            }
        }
        Err(e) => println!("\n\u{274C} FAILED: {}", e),
    }

    // Test 2: Without harness (comparison)
    println!("\n{}", "=".repeat(60));
    println!("TEST 2: Without Harness (comparison)");
    println!("{}", "=".repeat(60));

    match engine.run_yaml(YAML_WITHOUT_HARNESS, input.clone()).await {
        Ok(result) => {
            println!("\n\u{2705} SUCCESS");
            for (node_id, records) in &result.executions {
                if let Some(last) = records.last() {
                    if node_id == "translate" {
                        if let Some(sys) = last.outputs.get("_actual_system_prompt") {
                            println!("\n   System Prompt:\n   {}", sys);
                        }
                        if let Some(output) = last.outputs.get("output") {
                            println!("\n   Output: {}", output);
                        }
                    }
                }
            }
        }
        Err(e) => println!("\n\u{274C} FAILED: {}", e),
    }

    println!("\n{}", "=".repeat(60));
    println!("DONE");
    println!("{}", "=".repeat(60));
}
