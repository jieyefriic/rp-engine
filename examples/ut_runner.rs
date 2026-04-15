//! Universal test runner for ultimate-test workflows
//!
//! Usage: cargo run --example ut_runner -- <yaml_path> <input_json> <output_path>
//!
//! Example:
//!   cargo run --example ut_runner -- \
//!     ultimate-test/round-1/workflows/dim1-providers/p01_r1_openai_basic.yaml \
//!     '{"query": "Hello"}' \
//!     ultimate-test/round-1/results/dim1-providers/p01_r1_openai_basic.json

use riceprompt_engine::Engine;
use serde_json::{json, Value};
use std::time::Instant;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("Usage: {} <yaml_path> <input_json> <output_path>", args[0]);
        eprintln!("  yaml_path   - path to workflow YAML file");
        eprintln!("  input_json  - JSON string for workflow input");
        eprintln!("  output_path - path to save result JSON");
        std::process::exit(1);
    }

    let yaml_path = &args[1];
    let input_json_str = &args[2];
    let output_path = &args[3];

    // Read YAML
    let yaml = match std::fs::read_to_string(yaml_path) {
        Ok(y) => y,
        Err(e) => {
            let err = json!({
                "_runner_error": format!("Failed to read YAML file '{}': {}", yaml_path, e)
            });
            std::fs::write(output_path, serde_json::to_string_pretty(&err).unwrap()).unwrap();
            eprintln!("ERROR: {}", err);
            std::process::exit(1);
        }
    };

    // Parse input
    let input: Value = match serde_json::from_str(input_json_str) {
        Ok(v) => v,
        Err(e) => {
            let err = json!({
                "_runner_error": format!("Failed to parse input JSON: {}", e)
            });
            std::fs::write(output_path, serde_json::to_string_pretty(&err).unwrap()).unwrap();
            eprintln!("ERROR: {}", err);
            std::process::exit(1);
        }
    };

    // Build engine with all available providers from env vars
    let mut builder = Engine::builder();

    let providers = [
        ("openai",    "OPENAI_API_KEY"),
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("gemini",    "GEMINI_API_KEY"),
        ("xai",       "XAI_API_KEY"),
        ("deepseek",  "DEEPSEEK_API_KEY"),
        ("qwen",      "QWEN_API_KEY"),
        ("zhipu",     "ZHIPU_API_KEY"),
        ("moonshot",  "MOONSHOT_API_KEY"),
        ("minimax",   "MINIMAX_API_KEY"),
        ("huoshan",   "HUOSHAN_API_KEY"),
    ];

    let mut registered = Vec::new();
    for (name, env_var) in &providers {
        if let Ok(key) = std::env::var(env_var) {
            if !key.is_empty() {
                builder = builder.provider(name, &key);
                registered.push(*name);
            }
        }
    }

    eprintln!("Registered providers: {:?}", registered);

    let engine = match builder
        .capture_actual_prompt(true)
        .build()
        .await
    {
        Ok(e) => e,
        Err(e) => {
            let err = json!({
                "_runner_error": format!("Failed to build engine: {}", e),
                "_providers_registered": registered,
            });
            std::fs::write(output_path, serde_json::to_string_pretty(&err).unwrap()).unwrap();
            eprintln!("ENGINE BUILD ERROR: {}", e);
            std::process::exit(1);
        }
    };

    // Execute workflow
    let start = Instant::now();
    eprintln!("Running workflow: {}", yaml_path);

    let result = engine.run_yaml(&yaml, input).await;
    let elapsed = start.elapsed();

    // Build output
    let output = match result {
        Ok(exec_result) => {
            let result_json = serde_json::to_value(&exec_result).unwrap_or(json!({"_error": "serialize failed"}));
            json!({
                "_test_meta": {
                    "yaml_path": yaml_path,
                    "elapsed_ms": elapsed.as_millis() as u64,
                    "providers_registered": registered,
                    "status": format!("{:?}", exec_result.status),
                },
                "result": result_json,
            })
        }
        Err(e) => {
            json!({
                "_test_meta": {
                    "yaml_path": yaml_path,
                    "elapsed_ms": elapsed.as_millis() as u64,
                    "providers_registered": registered,
                    "status": "Error",
                },
                "_error": format!("{}", e),
                "_error_debug": format!("{:?}", e),
            })
        }
    };

    // Save result
    match std::fs::write(output_path, serde_json::to_string_pretty(&output).unwrap()) {
        Ok(_) => eprintln!("Result saved to: {}", output_path),
        Err(e) => eprintln!("Failed to save result: {}", e),
    }

    // Print summary to stderr
    let status = output.get("_test_meta")
        .and_then(|m| m.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("Unknown");
    eprintln!("Status: {} | Elapsed: {}ms", status, elapsed.as_millis());

    if let Some(err) = output.get("_error") {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}
