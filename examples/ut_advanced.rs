//! Specialized test runner for features that need more than run_yaml:
//! - Sub-workflow (workflow node) — needs workflows_dir
//! - Streaming — needs StreamHandler
//! - Checkpoint/Resume — needs two-step execution
//!
//! Usage: cargo run --example ut_advanced -- <test_name> <output_path>

use riceprompt_engine::{Engine, EngineBuilder, ExecutionStatus};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Collect streaming chunks
struct TestStreamHandler {
    chunks: Mutex<Vec<(String, String)>>, // (node_id, chunk)
}

impl TestStreamHandler {
    fn new() -> Self {
        Self { chunks: Mutex::new(Vec::new()) }
    }

    fn get_chunks(&self) -> Vec<(String, String)> {
        self.chunks.lock().unwrap().clone()
    }
}

impl riceprompt_engine::StreamHandler for TestStreamHandler {
    fn on_text_chunk(&self, node_id: &str, chunk: &str, _accumulated: &str) {
        self.chunks.lock().unwrap().push((node_id.to_string(), chunk.to_string()));
    }
}

fn env_key(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn build_engine_with_providers() -> EngineBuilder {
    let mut builder = Engine::builder();
    let providers = [
        ("openai", "OPENAI_API_KEY"), ("anthropic", "ANTHROPIC_API_KEY"),
        ("gemini", "GEMINI_API_KEY"), ("xai", "XAI_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"), ("qwen", "QWEN_API_KEY"),
        ("zhipu", "ZHIPU_API_KEY"), ("moonshot", "MOONSHOT_API_KEY"),
        ("minimax", "MINIMAX_API_KEY"), ("huoshan", "HUOSHAN_API_KEY"),
    ];
    for (name, env_var) in &providers {
        let key = env_key(env_var);
        if !key.is_empty() {
            builder = builder.provider(name, &key);
        }
    }
    builder
}

async fn test_streaming(output_path: &str) -> Value {
    let handler = Arc::new(TestStreamHandler::new());

    let engine = build_engine_with_providers()
        .capture_actual_prompt(true)
        .build().await.unwrap();

    let yaml = r#"
version: "1.0"
name: "Streaming Test"
nodes:
  - id: start
    type: start
  - id: gen
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: tpl
      stream: true
      variables:
        query: "start.query"
  - id: response
    type: response
    config:
      output:
        answer: "gen.output"
edges:
  - from: start
    to: gen
  - from: gen
    to: response
templates:
  tpl:
    user_prompt: "{{query}}"
    config:
      temperature: 0.5
      max_tokens: 100
"#;

    let start = Instant::now();
    let result = engine.run_yaml_with_stream_handler(
        yaml,
        json!({"query": "Count from 1 to 5, one number per line."}),
        handler.clone(),
    ).await;
    let elapsed = start.elapsed();

    let chunks = handler.get_chunks();
    match result {
        Ok(exec) => json!({
            "_test": "streaming",
            "status": format!("{:?}", exec.status),
            "elapsed_ms": elapsed.as_millis() as u64,
            "output": exec.output,
            "stream_chunks_count": chunks.len(),
            "stream_chunks_preview": chunks.iter().take(10).map(|(n, c)| format!("{}:{}", n, c)).collect::<Vec<_>>(),
            "has_ttft": exec.executions.get("gen").and_then(|r| r.first()).and_then(|r| r.outputs.get("_ttft_ms")).is_some(),
        }),
        Err(e) => json!({"_test": "streaming", "_error": e.to_string()}),
    }
}

async fn test_sub_workflow(output_path: &str) -> Value {
    // Create a temp directory with parent + child workflows
    let dir = tempfile::tempdir().unwrap();
    let wf_dir = dir.path().join("workflows");
    std::fs::create_dir(&wf_dir).unwrap();

    // Child workflow
    std::fs::write(wf_dir.join("child_greet.yaml"), r#"
version: "1.0"
name: "child_greet"
nodes:
  - id: start
    type: start
  - id: gen
    type: generate
    config:
      provider: qwen
      model: qwen-flash
      template: tpl
      variables:
        name: "start.name"
  - id: response
    type: response
    config:
      output:
        greeting: "gen.output"
edges:
  - from: start
    to: gen
  - from: gen
    to: response
templates:
  tpl:
    user_prompt: "Say hello to {{name}} in a creative way. One sentence only."
    config:
      temperature: 0.8
      max_tokens: 50
"#).unwrap();

    // Parent workflow that calls child
    std::fs::write(wf_dir.join("parent.yaml"), r#"
version: "1.0"
name: "parent"
nodes:
  - id: start
    type: start
  - id: call_child
    type: workflow
    config:
      workflow: "child_greet"
      variables:
        name: "start.user_name"
      output:
        child_greeting: "greeting"
  - id: enhance
    type: generate
    config:
      provider: qwen
      model: qwen-flash
      template: tpl_enhance
      variables:
        greeting: "call_child.child_greeting"
  - id: response
    type: response
    config:
      output:
        child_output: "call_child.child_greeting"
        enhanced: "enhance.output"
        child_status: "call_child._status"
        child_timing: "call_child._timing_ms"
edges:
  - from: start
    to: call_child
  - from: call_child
    to: enhance
  - from: enhance
    to: response
templates:
  tpl_enhance:
    user_prompt: "Make this greeting more enthusiastic: {{greeting}}"
    config:
      temperature: 0.9
      max_tokens: 80
"#).unwrap();

    let engine = build_engine_with_providers()
        .workflows_dir(&wf_dir)
        .capture_actual_prompt(true)
        .build().await.unwrap();

    let start = Instant::now();
    let result = engine.run("parent", json!({"user_name": "Alice"})).await;
    let elapsed = start.elapsed();

    match result {
        Ok(exec) => json!({
            "_test": "sub_workflow",
            "status": format!("{:?}", exec.status),
            "elapsed_ms": elapsed.as_millis() as u64,
            "output": exec.output,
            "nodes_executed": exec.executions.keys().collect::<Vec<_>>(),
        }),
        Err(e) => json!({"_test": "sub_workflow", "_error": e.to_string()}),
    }
}

async fn test_checkpoint_resume(output_path: &str) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let wf_dir = dir.path().join("workflows");
    std::fs::create_dir(&wf_dir).unwrap();

    std::fs::write(wf_dir.join("checkpoint_flow.yaml"), r#"
version: "1.0"
name: "checkpoint_flow"
nodes:
  - id: start
    type: start
  - id: gen_question
    type: generate
    config:
      provider: qwen
      model: qwen-flash
      template: tpl_question
      variables:
        topic: "start.topic"
  - id: pause
    type: checkpoint
    config:
      message: "Please provide your answer"
  - id: gen_evaluate
    type: generate
    config:
      provider: qwen
      model: qwen-flash
      template: tpl_evaluate
      variables:
        question: "gen_question.output"
        user_answer: "pause.user_answer"
  - id: response
    type: response
    config:
      output:
        question: "gen_question.output"
        evaluation: "gen_evaluate.output"
edges:
  - from: start
    to: gen_question
  - from: gen_question
    to: pause
  - from: pause
    to: gen_evaluate
  - from: gen_evaluate
    to: response
templates:
  tpl_question:
    user_prompt: "Ask a simple trivia question about {{topic}}. Just the question, nothing else."
    config:
      temperature: 0.7
      max_tokens: 50
  tpl_evaluate:
    user_prompt: |
      Question: {{question}}
      User answer: {{user_answer}}
      Is the answer correct? Respond with "Correct!" or "Incorrect, the answer is ..."
    config:
      temperature: 0.3
      max_tokens: 100
"#).unwrap();

    let engine = build_engine_with_providers()
        .workflows_dir(&wf_dir)
        .capture_actual_prompt(true)
        .build().await.unwrap();

    // Step 1: Run until checkpoint
    let start = Instant::now();
    let result1 = engine.run("checkpoint_flow", json!({"topic": "geography"})).await;
    let phase1_ms = start.elapsed().as_millis() as u64;

    let (checkpoint_state, question) = match &result1 {
        Ok(exec) if exec.status == ExecutionStatus::Paused => {
            let cs = exec.checkpoint_state.clone().unwrap();
            let q = exec.output.get("question").cloned().unwrap_or(Value::Null);
            (cs, q)
        }
        Ok(exec) => return json!({
            "_test": "checkpoint_resume",
            "_error": format!("Expected Paused, got {:?}", exec.status),
            "phase1_ms": phase1_ms,
            "output": exec.output,
        }),
        Err(e) => return json!({
            "_test": "checkpoint_resume",
            "_error": format!("Phase 1 failed: {}", e),
        }),
    };

    // Step 2: Resume with user answer
    let start2 = Instant::now();
    let result2 = engine.resume(
        "checkpoint_flow",
        checkpoint_state,
        "gen_evaluate",
        json!({"user_answer": "Paris"}),
    ).await;
    let phase2_ms = start2.elapsed().as_millis() as u64;

    match result2 {
        Ok(exec) => json!({
            "_test": "checkpoint_resume",
            "status": format!("{:?}", exec.status),
            "phase1_ms": phase1_ms,
            "phase2_ms": phase2_ms,
            "question": question,
            "output": exec.output,
            "nodes_executed": exec.executions.keys().collect::<Vec<_>>(),
        }),
        Err(e) => json!({
            "_test": "checkpoint_resume",
            "_error": format!("Phase 2 (resume) failed: {}", e),
            "phase1_ms": phase1_ms,
            "question": question,
        }),
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <test_name> <output_path>", args[0]);
        eprintln!("  test_name: streaming | sub_workflow | checkpoint_resume");
        std::process::exit(1);
    }

    let test_name = &args[1];
    let output_path = &args[2];

    eprintln!("Running advanced test: {}", test_name);

    let result = match test_name.as_str() {
        "streaming" => test_streaming(output_path).await,
        "sub_workflow" => test_sub_workflow(output_path).await,
        "checkpoint_resume" => test_checkpoint_resume(output_path).await,
        _ => {
            eprintln!("Unknown test: {}", test_name);
            std::process::exit(1);
        }
    };

    let json_str = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write(output_path, &json_str).unwrap();

    let status = result.get("status").and_then(|s| s.as_str()).unwrap_or("Unknown");
    let error = result.get("_error").and_then(|s| s.as_str());
    eprintln!("Status: {} | Error: {:?}", status, error);

    if error.is_some() {
        std::process::exit(1);
    }
}
