//! Integration tests for RicePrompt-Engine
//!
//! These tests use real YAML workflows and the Qwen API to verify
//! the engine works correctly for various graph patterns.

use std::collections::HashMap;
use std::sync::Arc;
use riceprompt_engine::{
    execute, ExecutionConfig, ExecutionStatus,
    QwenProvider, ProviderRegistry,
};
use serde_json::Value;

/// API key for Qwen (DashScope)
const QWEN_API_KEY: &str = "REDACTED_DASHSCOPE_KEY";

/// Create execution config with Qwen provider
fn setup() -> ExecutionConfig {
    let mut registry = ProviderRegistry::new();
    registry.register("qwen", Arc::new(QwenProvider::new(QWEN_API_KEY)));
    ExecutionConfig::default().with_providers(registry)
}

/// Test 1: Linear flow (start -> generate -> response)
#[tokio::test]
async fn test_linear_flow() {
    let yaml = include_str!("fixtures/linear.yaml");
    let mut input = HashMap::new();
    input.insert("query".to_string(), Value::String("什么是人工智能？".to_string()));

    let config = setup();
    let result = execute(yaml, input, config).await.unwrap();

    // Save result to JSON file
    let json = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/linear_result.json", &json).unwrap();
    println!("Saved to tests/output/linear_result.json");

    // Verify success
    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);

    // Verify think node was executed
    assert!(result.executions.contains_key("think"), "think node was not executed");

    // Verify output has answer
    assert!(result.output.contains_key("answer"), "output missing 'answer' field");
    let answer = result.output.get("answer").unwrap();
    assert!(!answer.is_null(), "answer should not be null");

    println!("Linear flow answer: {:?}", answer);
}

/// Test 2: Parallel flow (start -> [gen_chinese, gen_english] -> response)
#[tokio::test]
async fn test_parallel_flow() {
    let yaml = include_str!("fixtures/parallel.yaml");
    let mut input = HashMap::new();
    input.insert("topic".to_string(), Value::String("月球".to_string()));

    let config = setup();
    let result = execute(yaml, input, config).await.unwrap();

    // Save result to JSON file
    let json = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/parallel_result.json", &json).unwrap();
    println!("Saved to tests/output/parallel_result.json");

    // Verify success
    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);

    // Verify both generate nodes were executed
    assert!(result.executions.contains_key("gen_chinese"), "gen_chinese node was not executed");
    assert!(result.executions.contains_key("gen_english"), "gen_english node was not executed");

    // Both should be in same layer (parallel execution)
    let chinese_timing = result.timing.per_node.get("gen_chinese");
    let english_timing = result.timing.per_node.get("gen_english");
    assert!(chinese_timing.is_some() && english_timing.is_some(), "timing info missing");

    // Verify output has both results
    assert!(result.output.contains_key("chinese"), "output missing 'chinese' field");
    assert!(result.output.contains_key("english"), "output missing 'english' field");

    println!("Parallel flow - Chinese: {:?}", result.output.get("chinese"));
    println!("Parallel flow - English: {:?}", result.output.get("english"));
}

/// Test 3a: Conditional flow - math question
#[tokio::test]
async fn test_conditional_math() {
    let yaml = include_str!("fixtures/conditional.yaml");
    let mut input = HashMap::new();
    input.insert("question".to_string(), Value::String("1+1等于几？".to_string()));

    let config = setup();
    let result = execute(yaml, input, config).await.unwrap();

    // Save result to JSON file
    let json = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/conditional_math_result.json", &json).unwrap();
    println!("Saved to tests/output/conditional_math_result.json");

    // Verify success
    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);

    // Verify classifier was executed
    assert!(result.executions.contains_key("classifier"), "classifier node was not executed");

    // Check classification result
    if let Some(records) = result.executions.get("classifier") {
        if let Some(record) = records.last() {
            let category = record.outputs.get("category");
            println!("Classification result: {:?}", category);
        }
    }

    // Either math_solver or general_answer should be executed (depends on LLM classification)
    let math_executed = result.executions.contains_key("math_solver");
    let general_executed = result.executions.contains_key("general_answer");
    assert!(math_executed || general_executed, "Neither solver was executed");

    println!("Conditional (math) - math_solver executed: {}, general_answer executed: {}",
             math_executed, general_executed);
}

/// Test 3b: Conditional flow - general question
#[tokio::test]
async fn test_conditional_general() {
    let yaml = include_str!("fixtures/conditional.yaml");
    let mut input = HashMap::new();
    input.insert("question".to_string(), Value::String("今天天气怎么样？".to_string()));

    let config = setup();
    let result = execute(yaml, input, config).await.unwrap();

    // Save result to JSON file
    let json = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/conditional_general_result.json", &json).unwrap();
    println!("Saved to tests/output/conditional_general_result.json");

    // Verify success
    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);

    // Verify classifier was executed
    assert!(result.executions.contains_key("classifier"), "classifier node was not executed");

    // Check classification result
    if let Some(records) = result.executions.get("classifier") {
        if let Some(record) = records.last() {
            let category = record.outputs.get("category");
            println!("Classification result: {:?}", category);
        }
    }

    // Either math_solver or general_answer should be executed
    let math_executed = result.executions.contains_key("math_solver");
    let general_executed = result.executions.contains_key("general_answer");
    assert!(math_executed || general_executed, "Neither solver was executed");

    println!("Conditional (general) - math_solver executed: {}, general_answer executed: {}",
             math_executed, general_executed);
}

/// Test 4: Loop flow (think node loops until needs_more=false or max_iterations)
#[tokio::test]
async fn test_loop_flow() {
    let yaml = include_str!("fixtures/loop.yaml");
    let mut input = HashMap::new();
    input.insert("task".to_string(), Value::String("计算 2+3*4 的值，需要考虑运算优先级".to_string()));

    let config = setup();
    let result = execute(yaml, input, config).await.unwrap();

    // Save result to JSON file
    let json = serde_json::to_string_pretty(&result).unwrap();
    std::fs::write("tests/output/loop_result.json", &json).unwrap();
    println!("Saved to tests/output/loop_result.json");

    // Verify success
    assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);

    // Verify think node was executed
    assert!(result.executions.contains_key("think"), "think node was not executed");

    // Check number of iterations
    if let Some(records) = result.executions.get("think") {
        let iteration_count = records.len();
        println!("Loop flow - iterations: {}", iteration_count);

        // Should be between 1 and 4 (max_iterations=3 means up to 4 executions)
        assert!(iteration_count >= 1 && iteration_count <= 4,
                "Unexpected iteration count: {}", iteration_count);

        // Print each thought
        for (i, record) in records.iter().enumerate() {
            println!("  Iteration {} [{}]: thought={:?}",
                     i + 1,
                     record.short_id,
                     record.outputs.get("thought"));
        }
    }

    // Verify final output
    println!("Loop flow - final answer: {:?}", result.output.get("final"));
    println!("Loop flow - all thoughts: {:?}", result.output.get("all_thoughts"));
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
    let input = HashMap::new();
    let config = ExecutionConfig::default();

    let result = execute(yaml, input, config).await;
    assert!(result.is_err(), "Should fail validation without response node");
}

/// Test validation: workflow with invalid layer order should fail
#[tokio::test]
async fn test_validation_invalid_layer_order() {
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
    let input = HashMap::new();
    let config = ExecutionConfig::default();

    let result = execute(yaml, input, config).await;
    assert!(result.is_err(), "Should fail validation with invalid layer order");
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
    let input = HashMap::new();
    let config = ExecutionConfig::default();

    let result = execute(yaml, input, config).await;
    assert!(result.is_err(), "Should fail validation without provider");
}

/// Test: Supervisor workflow with TaskPlanner
#[tokio::test]
async fn test_supervisor_task_planner() {
    let yaml = include_str!("fixtures/TaskPlanner.yaml");
    let mut input = HashMap::new();
    input.insert("user_input".to_string(), Value::String("帮我查询用户表中所有活跃用户".to_string()));

    let config = setup();
    let result = execute(yaml, input, config).await;

    // Handle both success and error cases for debugging
    match result {
        Ok(result) => {
            // Save result to JSON file
            let json = serde_json::to_string_pretty(&result).unwrap();
            std::fs::write("tests/output/supervisor_task_planner_state.json", &json).unwrap();
            println!("Saved to tests/output/supervisor_task_planner_state.json");

            // Verify success
            assert_eq!(result.status, ExecutionStatus::Success, "Execution failed: {:?}", result.error);

            // Verify supervisor node was executed
            assert!(result.executions.contains_key("node_2"), "supervisor node was not executed");

            // Print supervisor output
            if let Some(records) = result.executions.get("node_2") {
                if let Some(record) = records.last() {
                    println!("Supervisor task_result: {:?}", record.outputs.get("task_result"));
                    println!("Supervisor exit_reason: {:?}", record.outputs.get("exit_reason"));
                }
            }

            // Print final output
            println!("Final output: {:?}", result.output);
        }
        Err(e) => {
            println!("Execution error: {:?}", e);
            // Save error info for debugging
            let error_json = serde_json::json!({
                "error": e.to_string(),
                "error_type": format!("{:?}", e)
            });
            std::fs::write(
                "tests/output/supervisor_task_planner_error.json",
                serde_json::to_string_pretty(&error_json).unwrap()
            ).unwrap();
            println!("Saved error to tests/output/supervisor_task_planner_error.json");

            // For now, we expect this to potentially fail due to LLM response parsing
            // Comment out the panic to see what happens
            panic!("Supervisor test failed: {}", e);
        }
    }
}
