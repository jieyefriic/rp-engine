//! Integration tests for RicePrompt-Engine
//!
//! These tests use real YAML workflows and the Qwen API to verify
//! the engine works correctly for various graph patterns.
//! They require network access and a valid QWEN_API_KEY.

use std::sync::Arc;
use riceprompt_engine::{
    Engine, ExecutionStatus,
};
use serde_json::{json, Value};

/// API key for Qwen (DashScope)
const QWEN_API_KEY: &str = "sk-2a516181a8c6431888fe5312a0f23e46";

/// Create an engine with Qwen provider
async fn setup_engine() -> Engine {
    Engine::builder()
        .provider("qwen", QWEN_API_KEY)
        .build()
        .await
        .unwrap()
}

/// Test 1: Linear flow (start -> generate -> response)
#[tokio::test]
async fn test_linear_flow() {
    let yaml = include_str!("fixtures/linear.yaml");
    let engine = setup_engine().await;
    let result = engine.run_yaml(yaml, json!({
        "query": "什么是人工智能？"
    })).await.unwrap();

    // Save result to JSON file
    let json_str = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/linear_result.json", &json_str).unwrap();

    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);
    assert!(result.executions.contains_key("think"), "think node was not executed");
    assert!(result.output.contains_key("answer"), "output missing 'answer' field");
    println!("Linear flow answer: {:?}", result.output.get("answer"));
}

/// Test 2: Parallel flow (start -> [gen_chinese, gen_english] -> response)
#[tokio::test]
async fn test_parallel_flow() {
    let yaml = include_str!("fixtures/parallel.yaml");
    let engine = setup_engine().await;
    let result = engine.run_yaml(yaml, json!({
        "topic": "月球"
    })).await.unwrap();

    let json_str = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/parallel_result.json", &json_str).unwrap();

    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);
    assert!(result.executions.contains_key("gen_chinese"), "gen_chinese node was not executed");
    assert!(result.executions.contains_key("gen_english"), "gen_english node was not executed");
    assert!(result.output.contains_key("chinese"), "output missing 'chinese' field");
    assert!(result.output.contains_key("english"), "output missing 'english' field");
}

/// Test 3a: Conditional flow - math question
#[tokio::test]
async fn test_conditional_math() {
    let yaml = include_str!("fixtures/conditional.yaml");
    let engine = setup_engine().await;
    let result = engine.run_yaml(yaml, json!({
        "question": "1+1等于几？"
    })).await.unwrap();

    let json_str = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/conditional_math_result.json", &json_str).unwrap();

    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);
    assert!(result.executions.contains_key("classifier"), "classifier node was not executed");

    let math_executed = result.executions.contains_key("math_solver");
    let general_executed = result.executions.contains_key("general_answer");
    assert!(math_executed || general_executed, "Neither solver was executed");
}

/// Test 3b: Conditional flow - general question
#[tokio::test]
async fn test_conditional_general() {
    let yaml = include_str!("fixtures/conditional.yaml");
    let engine = setup_engine().await;
    let result = engine.run_yaml(yaml, json!({
        "question": "今天天气怎么样？"
    })).await.unwrap();

    let json_str = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/conditional_general_result.json", &json_str).unwrap();

    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);
    assert!(result.executions.contains_key("classifier"), "classifier node was not executed");

    let math_executed = result.executions.contains_key("math_solver");
    let general_executed = result.executions.contains_key("general_answer");
    assert!(math_executed || general_executed, "Neither solver was executed");
}

/// Test 4: Loop flow (think node loops until needs_more=false or max_iterations)
#[tokio::test]
async fn test_loop_flow() {
    let yaml = include_str!("fixtures/loop.yaml");
    let engine = setup_engine().await;
    let result = engine.run_yaml(yaml, json!({
        "task": "计算 2+3*4 的值，需要考虑运算优先级"
    })).await.unwrap();

    let json_str = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/loop_result.json", &json_str).unwrap();

    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);
    assert!(result.executions.contains_key("think"), "think node was not executed");

    if let Some(records) = result.executions.get("think") {
        let iteration_count = records.len();
        assert!(iteration_count >= 1 && iteration_count <= 4,
                "Unexpected iteration count: {}", iteration_count);
    }
}

/// Test validation: workflow without response node should fail
#[tokio::test]
async fn test_validation_missing_response() {
    let yaml = r#"
version: "0.1"
name: "Invalid"
nodes:
  - id: start
    type: start
    layer: 0
edges: []
"#;
    let engine = setup_engine().await;
    let result = engine.run_yaml(yaml, json!({})).await;
    assert!(result.is_err(), "Should fail validation without response node");
}

/// Test validation: workflow with backward edge (protected by max_iterations)
#[tokio::test]
async fn test_validation_backward_edge() {
    let yaml = r#"
version: "0.1"
name: "Invalid"
nodes:
  - id: start
    type: start
    layer: 0
  - id: gen
    type: generate
    layer: 1
    config:
      provider: qwen
      model: qwen-turbo
  - id: response
    type: response
    layer: 2
edges:
  - from: start
    to: gen
  - from: gen
    to: start
  - from: gen
    to: response
"#;
    let engine = setup_engine().await;
    // Backward edges are allowed but protected by max_iterations
    let result = engine.run_yaml(yaml, json!({})).await;
    // This may succeed or fail depending on iteration limits, but should not panic
    let _ = result;
}

/// Test validation: generate node without provider should fail
#[tokio::test]
async fn test_validation_generate_without_provider() {
    let yaml = r#"
version: "0.1"
name: "Invalid"
nodes:
  - id: start
    type: start
    layer: 0
  - id: gen
    type: generate
    layer: 1
    config:
      model: qwen-turbo
  - id: response
    type: response
    layer: 2
edges:
  - from: start
    to: gen
  - from: gen
    to: response
"#;
    let engine = setup_engine().await;
    let result = engine.run_yaml(yaml, json!({})).await;
    assert!(result.is_err(), "Should fail validation without provider");
}

/// Test: Supervisor workflow with TaskPlanner
#[tokio::test]
async fn test_supervisor_task_planner() {
    let yaml = include_str!("fixtures/TaskPlanner.yaml");
    let engine = setup_engine().await;
    let result = engine.run_yaml(yaml, json!({
        "user_input": "帮我查询用户表中所有活跃用户"
    })).await;

    match result {
        Ok(result) => {
            let json_str = serde_json::to_string_pretty(&result).unwrap();
            std::fs::write("tests/output/supervisor_task_planner_state.json", &json_str).unwrap();
            assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);
            assert!(result.executions.contains_key("node_2"), "supervisor node was not executed");
        }
        Err(e) => {
            let error_json = serde_json::json!({
                "error": e.to_string(),
                "error_type": format!("{:?}", e)
            });
            std::fs::write(
                "tests/output/supervisor_task_planner_error.json",
                serde_json::to_string_pretty(&error_json).unwrap()
            ).unwrap();
            panic!("Supervisor test failed: {}", e);
        }
    }
}
