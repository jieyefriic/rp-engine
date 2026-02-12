//! Test frontend exported YAML
//!
//! Run: cargo run --example test_frontend_yaml

use riceprompt_engine::Engine;
use serde_json::json;

const QWEN_API_KEY: &str = "sk-2a516181a8c6431888fe5312a0f23e46";

const YAML: &str = r#"
version: "0.1"
name: New Workflow
input:
  fields:
    - name: input
      type: string
      required: true
      description: User input
nodes:
  - id: node_1
    type: start
    layer: 0
    config:
      output:
        input: input.input
  - id: node_2
    type: generate
    layer: 1
    config:
      template: tpl_mk0nmasr_59j6b
      provider: qwen
      model: qwen-flash
      variables:
        user_input: node_1.input
  - id: node_3
    type: generate
    layer: 2
    config:
      template: tpl_mk0pi2k0_zcz3a
      provider: qwen
      model: qwen-flash
      variables:
        user_input: node_1.input
        intention: node_2.intention
  - id: node_4
    type: generate
    layer: 3
    config:
      template: tpl_mk0pevby_lm1ej
      provider: qwen
      model: qwen-flash
      variables:
        user_input: node_1.input
  - id: node_5
    type: response
    layer: 4
    config:
      output:
        input_8: node_3.response
edges:
  - from: node_1
    to: node_2
    is_default: true
  - from: node_2
    to: node_3
    condition: node_2.confidence_level > 0.5
  - from: node_2
    to: node_4
    condition: node_2.confidence_level <= 0.5
  - from: node_3
    to: node_5
    is_default: true
  - from: node_4
    to: node_5
    is_default: true
templates:
  tpl_mk0nmasr_59j6b:
    user_prompt: "Please identify below user intention: {{user_input}}"
    output_schema:
      - name: intention
        type: string
        example: The user's intention is to...
      - name: reason
        type: string
        example: The reason for this intention is...
      - name: confidence_level
        type: number
        example: "0.85"
  tpl_mk0pi2k0_zcz3a:
    user_prompt: Given the {{user_input}} the {{intention}} identified, give a response to user.
    output_schema:
      - name: response
        type: string
        example: Nature lanuage response
  tpl_mk0pevby_lm1ej:
    user_prompt: The below user input has a problem on identifying the intention of {{user_input}}. Please further analyze this input and give an instruction for the response.
    output_schema:
      - name: instruction
        type: string
        example: Please clarify the user's intention regarding their request.
"#;

#[tokio::main]
async fn main() {
    println!("Testing frontend exported YAML...\n");

    let engine = Engine::builder()
        .provider("qwen", QWEN_API_KEY)
        .build()
        .await
        .unwrap();

    println!("Input: {{\"input\": \"我好讨厌你啊\"}}\n");

    match engine.run_yaml(YAML, json!({"input": "我好讨厌你啊"})).await {
        Ok(result) => {
            println!("Execution succeeded!\n");
            let json = serde_json::to_string_pretty(&result).unwrap();
            println!("{}", json);
        }
        Err(e) => {
            println!("Execution failed!");
            println!("Error: {:?}", e);
        }
    }
}
