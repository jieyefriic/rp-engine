//! Generic demo-yaml runner.
//!
//! Run: cargo run --example run_demo -- <yaml_path> <input_json> [out_json]
//!
//! Reads QWEN_API_KEY (and any other PROVIDER_API_KEY env vars listed below)
//! and executes the workflow, writing the full ExecutionResult JSON to disk
//! so the frontend Inspector can load it.

use riceprompt_engine::Engine;
use std::env;
use std::fs;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: run_demo <yaml_path> <input_json> [out_json]");
        std::process::exit(2);
    }
    let yaml_path = &args[1];
    let input_json = &args[2];
    let out_path = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "demo_result.json".to_string());

    let yaml = fs::read_to_string(yaml_path).expect("read yaml");
    let input: serde_json::Value = serde_json::from_str(input_json).expect("parse input json");

    let mut builder = Engine::builder();
    for (env_key, provider_id) in [
        ("QWEN_API_KEY", "qwen"),
        ("OPENAI_API_KEY", "openai"),
        ("ANTHROPIC_API_KEY", "anthropic"),
        ("GEMINI_API_KEY", "gemini"),
        ("XAI_API_KEY", "xai"),
        ("DEEPSEEK_API_KEY", "deepseek"),
        ("ZHIPU_API_KEY", "zhipu"),
        ("MOONSHOT_API_KEY", "moonshot"),
        ("MINIMAX_API_KEY", "minimax"),
        ("HUOSHAN_API_KEY", "huoshan"),
    ] {
        if let Ok(k) = env::var(env_key) {
            builder = builder.provider(provider_id, &k);
            eprintln!("[run_demo] provider key loaded: {}", provider_id);
        }
    }

    let engine = builder.build().await.expect("build engine");

    eprintln!("[run_demo] executing {} ...", yaml_path);
    let started = std::time::Instant::now();
    let result = engine
        .run_yaml(&yaml, input)
        .await
        .expect("run_yaml");
    eprintln!("[run_demo] done in {} ms", started.elapsed().as_millis());

    let pretty = serde_json::to_string_pretty(&result).expect("serialize");
    fs::write(&out_path, &pretty).expect("write out json");

    eprintln!("[run_demo] wrote {} ({} bytes)", out_path, pretty.len());
    eprintln!(
        "[run_demo] status={:?} total_cost=${:.6} unknown_pricing={}",
        result.status, result.cost.total_cost, result.cost.has_unknown_pricing
    );
}
