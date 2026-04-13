//! Test MCP node with a public MCP server
//!
//! Run with: cargo run --example test_mcp
//!
//! Uses DeepWiki's public MCP server (no API key required)

use riceprompt_engine::Engine;
use serde_json::json;

const YAML_MCP: &str = r#"
version: "1.0"
name: "test_mcp"
description: "Test MCP node with DeepWiki public server"

mcp_servers:
  deepwiki:
    url: "https://mcp.deepwiki.com/mcp"

nodes:
  - id: start
    type: start

  - id: wiki_read
    type: mcp
    config:
      server: deepwiki
      tool: read_wiki_structure
      variables:
        repoName: "anthropics/anthropic-cookbook"

  - id: response
    type: response
    config:
      output:
        result: "wiki_read.result"
        timing: "wiki_read._timing_ms"

edges:
  - from: start
    to: wiki_read
  - from: wiki_read
    to: response
"#;

#[tokio::main]
async fn main() {
    let engine = Engine::builder()
        .build()
        .await
        .unwrap();

    println!("{}", "=".repeat(60));
    println!("TEST: MCP Node with DeepWiki public server");
    println!("{}", "=".repeat(60));

    let input = json!({});

    match engine.run_yaml(YAML_MCP, input).await {
        Ok(result) => {
            println!("\n\u{2705} SUCCESS");
            println!("   Status: {:?}", result.status);

            // Check MCP node output
            if let Some(records) = result.executions.get("wiki_read") {
                if let Some(last) = records.last() {
                    if let Some(error) = last.outputs.get("_error") {
                        if error == &serde_json::Value::Bool(true) {
                            let msg = last.outputs.get("_error_message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            println!("   \u{274c} MCP tool call error: {}", msg);
                        }
                    } else {
                        println!("   \u{2705} MCP tool call succeeded");
                    }

                    if let Some(timing) = last.outputs.get("_timing_ms") {
                        println!("   Timing: {}ms", timing);
                    }

                    if let Some(result_val) = last.outputs.get("result") {
                        let result_str = serde_json::to_string_pretty(result_val).unwrap_or_default();
                        println!("   Result (first 500 chars):");
                        println!("   {}", &result_str[..result_str.len().min(500)]);
                    }
                }
            }

            // Final output
            println!("\n   Workflow output keys: {:?}", result.output.keys().collect::<Vec<_>>());
        }
        Err(e) => {
            println!("\n\u{274c} FAILED: {}", e);
        }
    }

    // Test 2: Scenario B — mcp_tools → generate (LLM-driven tool calling)
    println!("\n{}", "=".repeat(60));
    println!("TEST 2: Scenario B — mcp_tools + generate (LLM-driven)");
    println!("{}", "=".repeat(60));

    let key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("\n   Skipping: DEEPSEEK_API_KEY not set");
            println!("\n{}", "=".repeat(60));
            println!("DONE");
            println!("{}", "=".repeat(60));
            return;
        }
    };

    let yaml_b = r#"
version: "1.0"
name: "test_mcp_scenario_b"
description: "Test LLM-driven MCP tool calling"

mcp_servers:
  deepwiki:
    url: "https://mcp.deepwiki.com/mcp"

nodes:
  - id: start
    type: start

  - id: wiki_tools
    type: mcp_tools
    config:
      server: deepwiki

  - id: agent
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: agent_tpl
      mcp_tool_sets: ["wiki_tools"]
      variables:
        question: "start.question"

  - id: response
    type: response
    config:
      output:
        answer: "agent.output"

edges:
  - from: start
    to: wiki_tools
  - from: wiki_tools
    to: agent
  - from: agent
    to: response

templates:
  agent_tpl:
    user_prompt: "{{question}}"
    system_prompt: "You have access to DeepWiki tools. Use them to answer the question. Be concise."
    config:
      temperature: 0.3
      max_tokens: 1000
"#;

    let engine_b = Engine::builder()
        .provider("deepseek", &key)
        .capture_actual_prompt(true)
        .build()
        .await
        .unwrap();

    let input_b = json!({
        "question": "List the topics in the anthropics/anthropic-cookbook wiki. Be brief."
    });

    match engine_b.run_yaml(yaml_b, input_b).await {
        Ok(result) => {
            println!("\n\u{2705} SUCCESS");

            // Check mcp_tools node discovered tools
            if let Some(records) = result.executions.get("wiki_tools") {
                if let Some(last) = records.last() {
                    if let Some(count) = last.outputs.get("_tool_count") {
                        println!("   MCP tools discovered: {}", count);
                    }
                }
            }

            // Check generate node output
            if let Some(records) = result.executions.get("agent") {
                if let Some(last) = records.last() {
                    if let Some(output) = last.outputs.get("output") {
                        let s = output.to_string();
                        println!("   Agent output (first 300 chars):");
                        println!("   {}", &s[..s.len().min(300)]);
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
