//! Test harness post-execution memory writeback
//!
//! Run with: DEEPSEEK_API_KEY=sk-xxx cargo run --example test_harness_writeback

use riceprompt_engine::Engine;
use serde_json::json;
use std::io::Write;

fn get_key(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("Missing env var: {}. Set it before running.", name);
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() {
    let key = get_key("DEEPSEEK_API_KEY");

    // Create a temp memory file
    let temp_dir = std::env::temp_dir();
    let memory_path = temp_dir.join("harness_test_memory.txt");

    // Pre-populate with some initial memory
    {
        let mut f = std::fs::File::create(&memory_path).unwrap();
        writeln!(f, "User prefers concise answers.").unwrap();
        writeln!(f, "User works with Rust and TypeScript.").unwrap();
    }

    println!("Memory file: {}", memory_path.display());
    println!("Initial memory:\n  {}", std::fs::read_to_string(&memory_path).unwrap().trim());

    let yaml = format!(r#"
version: "1.0"
name: "test_writeback"
description: "Test harness memory writeback"

harness:
  instructions: |
    You are a helpful coding assistant.
    Always be concise and precise.
  memory_file: "{memory_path}"
  writeback:
    provider: deepseek
    model: deepseek-chat
    memory_char_limit: 500

nodes:
  - id: start
    type: start

  - id: helper
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: helper_tpl
      variables:
        question: "start.question"

  - id: response
    type: response
    config:
      output:
        answer: "helper.output"

edges:
  - from: start
    to: helper
  - from: helper
    to: response

templates:
  helper_tpl:
    user_prompt: "{{question}}"
    config:
      temperature: 0.3
      max_tokens: 300
"#, memory_path = memory_path.display());

    let engine = Engine::builder()
        .provider("deepseek", &key)
        .capture_actual_prompt(true)
        .build()
        .await
        .unwrap();

    // Test 1: Run workflow — should trigger writeback
    println!("\n{}", "=".repeat(60));
    println!("TEST 1: Run workflow with writeback");
    println!("{}", "=".repeat(60));

    let input = json!({
        "question": "What's the best way to handle errors in Rust? I prefer using anyhow for applications."
    });

    match engine.run_yaml(&yaml, input).await {
        Ok(result) => {
            println!("\n\u{2705} Workflow completed");

            // Check system prompt had memory
            for (node_id, records) in &result.executions {
                if let Some(last) = records.last() {
                    if node_id == "helper" {
                        if let Some(sys) = last.outputs.get("_actual_system_prompt") {
                            let sys_str = sys.to_string();
                            if sys_str.contains("Memory") {
                                println!("   \u{2705} Memory was in system_prompt");
                            }
                            if sys_str.contains("concise answers") {
                                println!("   \u{2705} Initial memory content was injected");
                            }
                        }
                    }
                }
            }

            // Check if memory file was updated
            let updated_memory = std::fs::read_to_string(&memory_path).unwrap();
            println!("\n   Updated memory ({} chars):", updated_memory.len());
            println!("   {}", updated_memory.trim());

            if updated_memory != "User prefers concise answers.\nUser works with Rust and TypeScript.\n" {
                println!("\n   \u{2705} Memory file WAS UPDATED by writeback");
            } else {
                println!("\n   \u{274c} Memory file was NOT updated");
            }

            if updated_memory.len() <= 500 {
                println!("   \u{2705} Memory within char limit (500)");
            } else {
                println!("   \u{274c} Memory exceeds char limit: {} > 500", updated_memory.len());
            }
        }
        Err(e) => println!("\n\u{274c} FAILED: {}", e),
    }

    // Test 2: Run again — memory should now contain updated info
    println!("\n{}", "=".repeat(60));
    println!("TEST 2: Run again — verify updated memory is injected");
    println!("{}", "=".repeat(60));

    let input2 = json!({
        "question": "How do I read a file in Rust?"
    });

    match engine.run_yaml(&yaml, input2).await {
        Ok(result) => {
            println!("\n\u{2705} Second run completed");
            for (node_id, records) in &result.executions {
                if let Some(last) = records.last() {
                    if node_id == "helper" {
                        if let Some(sys) = last.outputs.get("_actual_system_prompt") {
                            let sys_str = sys.to_string();
                            println!("\n   System prompt (first 400 chars):");
                            println!("   {}", &sys_str[..sys_str.len().min(400)]);
                        }
                    }
                }
            }

            let final_memory = std::fs::read_to_string(&memory_path).unwrap();
            println!("\n   Final memory ({} chars):", final_memory.len());
            println!("   {}", final_memory.trim());
        }
        Err(e) => println!("\n\u{274c} FAILED: {}", e),
    }

    // Cleanup
    let _ = std::fs::remove_file(&memory_path);

    println!("\n{}", "=".repeat(60));
    println!("DONE");
    println!("{}", "=".repeat(60));
}
