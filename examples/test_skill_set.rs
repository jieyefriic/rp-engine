//! Test skill_set node with progressive disclosure via tool calling
//!
//! Run with: DEEPSEEK_API_KEY=sk-xxx cargo run --example test_skill_set

use riceprompt_engine::Engine;
use serde_json::json;

fn get_key(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("Missing env var: {}. Set it before running.", name);
        std::process::exit(1);
    })
}

const YAML_WITH_SKILLS: &str = r#"
version: "1.0"
name: "test_skill_set"
description: "Test skill_set node with progressive disclosure"

nodes:
  - id: start
    type: start

  - id: coding_skills
    type: skill_set
    config:
      skills:
        - name: rust_conventions
          description: "Rust coding conventions and best practices for idiomatic code"
          content: |
            ## Rust Conventions Skill
            When writing Rust code, follow these rules:
            1. Use snake_case for variables and functions
            2. Use CamelCase for types and traits
            3. Prefer &str over String for function parameters
            4. Use Result<T, E> for fallible operations, never panic
            5. Implement Display for user-facing errors
            6. Use #[derive(Debug)] on all public types
            7. Prefer iterators over explicit loops
            8. Use clippy and rustfmt
        - name: api_design
          description: "REST API design guidelines including naming, versioning, and error handling"
          content: |
            ## API Design Skill
            When designing REST APIs:
            1. Use plural nouns for resources: /users, /orders
            2. Version APIs in URL: /v1/users
            3. Use HTTP status codes correctly (200, 201, 400, 404, 500)
            4. Return consistent error format: {"error": "message", "code": "ERROR_CODE"}
            5. Use pagination for list endpoints
            6. Document with OpenAPI/Swagger

  - id: helper
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: helper_tpl
      skill_sets: ["coding_skills"]
      variables:
        question: "start.question"

  - id: response
    type: response
    config:
      output:
        answer: "helper.output"

edges:
  - from: start
    to: coding_skills
  - from: coding_skills
    to: helper
  - from: helper
    to: response

templates:
  helper_tpl:
    user_prompt: |
      Answer the following question. If you need detailed guidelines,
      use the load_skill tool to load the relevant skill first.

      Question: {{question}}
    config:
      temperature: 0.3
      max_tokens: 1000
"#;

const YAML_WITHOUT_SKILLS: &str = r#"
version: "1.0"
name: "test_no_skills"
description: "Same workflow without skills for comparison"

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
    user_prompt: "Answer: {{question}}"
    config:
      temperature: 0.3
      max_tokens: 500
"#;

#[tokio::main]
async fn main() {
    let key = get_key("DEEPSEEK_API_KEY");
    let input = json!({
        "question": "What are the naming conventions for Rust functions and types? Give a brief answer."
    });

    let engine = Engine::builder()
        .provider("deepseek", &key)
        .capture_actual_prompt(true)
        .build()
        .await
        .unwrap();

    // Test 1: With skill_set
    println!("{}", "=".repeat(60));
    println!("TEST 1: With Skill Set (progressive disclosure)");
    println!("{}", "=".repeat(60));

    match engine.run_yaml(YAML_WITH_SKILLS, input.clone()).await {
        Ok(result) => {
            println!("\n\u{2705} SUCCESS");
            for (node_id, records) in &result.executions {
                if let Some(last) = records.last() {
                    if node_id == "coding_skills" {
                        // Verify skill_set node output
                        if let Some(count) = last.outputs.get("_skill_count") {
                            println!("\n   Skill set resolved: {} skills", count);
                        }
                    }
                    if node_id == "helper" {
                        // Check system prompt contains skill metadata
                        if let Some(sys) = last.outputs.get("_actual_system_prompt") {
                            let sys_str = sys.to_string();
                            println!("\n   System Prompt (first 300 chars):");
                            println!("   {}", &sys_str[..sys_str.len().min(300)]);

                            if sys_str.contains("Available Skills") {
                                println!("\n   \u{2705} Skill metadata FOUND in system_prompt");
                            } else {
                                println!("\n   \u{274c} Skill metadata NOT found in system_prompt");
                            }
                            if sys_str.contains("rust_conventions") {
                                println!("   \u{2705} rust_conventions skill listed");
                            }
                            if sys_str.contains("api_design") {
                                println!("   \u{2705} api_design skill listed");
                            }
                        }
                        if let Some(output) = last.outputs.get("output") {
                            println!("\n   Output: {}", &output.to_string()[..output.to_string().len().min(200)]);
                        }
                    }
                }
            }
        }
        Err(e) => println!("\n\u{274c} FAILED: {}", e),
    }

    // Test 2: Without skills (comparison)
    println!("\n{}", "=".repeat(60));
    println!("TEST 2: Without Skills (comparison)");
    println!("{}", "=".repeat(60));

    match engine.run_yaml(YAML_WITHOUT_SKILLS, input.clone()).await {
        Ok(result) => {
            println!("\n\u{2705} SUCCESS");
            for (node_id, records) in &result.executions {
                if let Some(last) = records.last() {
                    if node_id == "helper" {
                        if let Some(sys) = last.outputs.get("_actual_system_prompt") {
                            println!("\n   System Prompt: {}", sys);
                        } else {
                            println!("\n   No system prompt (expected for no-skill workflow)");
                        }
                        if let Some(output) = last.outputs.get("output") {
                            println!("\n   Output: {}", &output.to_string()[..output.to_string().len().min(200)]);
                        }
                    }
                }
            }
        }
        Err(e) => println!("\n\u{274c} FAILED: {}", e),
    }

    println!("\n{}", "=".repeat(60));
    println!("DONE");
    println!("{}", "=".repeat(60));
}
