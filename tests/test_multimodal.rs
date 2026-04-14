//! Integration test for multimodal image pipeline across 4 providers
//!
//! Tests three image input formats:
//! 1. Base64 (raw) — auto-detect media type via magic bytes
//! 2. Data URI — data:image/jpeg;base64,...
//! 3. HTTP URL — direct URL passthrough
//!
//! Providers: OpenAI, Anthropic, Gemini, Qwen

use riceprompt_engine::{Engine, EngineBuilder};
use serde_json::json;
use std::path::Path;
use tempfile::tempdir;

fn env_key(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{} env var must be set", name))
}

/// Helper: read image file and return base64 string
fn read_image_base64(path: &str) -> String {
    let bytes = std::fs::read(path).expect("Failed to read image file");
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

/// Helper: build a simple VL workflow YAML
fn vl_workflow_yaml() -> String {
    r#"
version: "1.0"
name: "VL Test"
nodes:
  - id: start
    type: start
  - id: describe
    type: generate
    config:
      provider: __PROVIDER__
      model: __MODEL__
      max_tokens: 200
      temperature: 0.3
      variables:
        user_message: start.user_message
      images:
        - start.image_data
  - id: response
    type: response
    config:
      output:
        text: describe.output
edges:
  - from: start
    to: describe
  - from: describe
    to: response
templates:
  describe:
    user_prompt: "{{user_message}}"
"#.to_string()
}

/// Write a workflow YAML file for a specific provider/model
fn write_workflow(dir: &Path, name: &str, provider: &str, model: &str) {
    let yaml = vl_workflow_yaml()
        .replace("__PROVIDER__", provider)
        .replace("__MODEL__", model);
    std::fs::write(dir.join(format!("{}.yaml", name)), yaml).unwrap();
}

/// Run VL test for a single provider
async fn run_vl_test(
    provider_name: &str,
    api_key: &str,
    model: &str,
    image_data: &str,
) -> Result<String, String> {
    let dir = tempdir().unwrap();
    let workflows_dir = dir.path().join("workflows");
    std::fs::create_dir(&workflows_dir).unwrap();

    write_workflow(&workflows_dir, "vl_test", provider_name, model);

    let engine = EngineBuilder::new()
        .workflows_dir(&workflows_dir)
        .provider(provider_name, api_key)
        .build()
        .await
        .map_err(|e| format!("Build error: {}", e))?;

    let result = engine.run("vl_test", json!({
        "user_message": "Describe this image in one sentence. What do you see?",
        "image_data": image_data,
    })).await.map_err(|e| format!("Run error: {}", e))?;

    // Debug output
    println!("  Status: {:?}", result.status);
    println!("  Output keys: {:?}", result.output.keys().collect::<Vec<_>>());
    if let Some(err) = &result.error {
        println!("  Error: {}", err);
    }
    for (node_id, records) in &result.executions {
        for rec in records {
            println!("  Node '{}': output keys = {:?}", node_id, rec.outputs.keys().collect::<Vec<_>>());
            // Print first 200 chars of text output if present
            if let Some(text_val) = rec.outputs.get("text") {
                let s = text_val.as_str().unwrap_or("(non-string)");
                println!("    text = {:?}", &s[..s.len().min(200)]);
            }
        }
    }

    let output_text = result.output
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(output_text)
}

#[tokio::test]
async fn test_openai_base64() {
    let api_key = env_key("OPENAI_API_KEY");
    let b64 = read_image_base64("/Users/jieye/Downloads/IMG_0C360C6B7B8A-1.jpeg");

    println!("\n=== OpenAI (gpt-4o-mini) — raw base64 ===");
    match run_vl_test("openai", api_key, "gpt-4o-mini", &b64).await {
        Ok(text) => {
            println!("Response: {}", text);
            assert!(!text.is_empty(), "OpenAI returned empty response");
        }
        Err(e) => panic!("OpenAI failed: {}", e),
    }
}

#[tokio::test]
async fn test_openai_data_uri() {
    let api_key = env_key("OPENAI_API_KEY");
    let b64 = read_image_base64("/Users/jieye/Downloads/IMG_0C360C6B7B8A-1.jpeg");
    let data_uri = format!("data:image/jpeg;base64,{}", b64);

    println!("\n=== OpenAI (gpt-4o-mini) — data URI ===");
    match run_vl_test("openai", api_key, "gpt-4o-mini", &data_uri).await {
        Ok(text) => {
            println!("Response: {}", text);
            assert!(!text.is_empty(), "OpenAI data URI returned empty response");
        }
        Err(e) => panic!("OpenAI data URI failed: {}", e),
    }
}

#[tokio::test]
async fn test_anthropic_base64() {
    let api_key = env_key("ANTHROPIC_API_KEY");
    let b64 = read_image_base64("/Users/jieye/Downloads/IMG_0C360C6B7B8A-1.jpeg");

    println!("\n=== Anthropic (claude-sonnet-4-20250514) — raw base64 ===");
    match run_vl_test("anthropic", api_key, "claude-sonnet-4-20250514", &b64).await {
        Ok(text) => {
            println!("Response: {}", text);
            assert!(!text.is_empty(), "Anthropic returned empty response");
        }
        Err(e) => panic!("Anthropic failed: {}", e),
    }
}

#[tokio::test]
async fn test_anthropic_url() {
    let api_key = env_key("ANTHROPIC_API_KEY");
    // Use a well-known public image URL
    let url = "https://upload.wikimedia.org/wikipedia/commons/thumb/4/47/PNG_transparency_demonstration_1.png/280px-PNG_transparency_demonstration_1.png";

    println!("\n=== Anthropic (claude-sonnet-4-20250514) — URL ===");
    match run_vl_test("anthropic", api_key, "claude-sonnet-4-20250514", url).await {
        Ok(text) => {
            println!("Response: {}", text);
            assert!(!text.is_empty(), "Anthropic URL returned empty response");
        }
        Err(e) => panic!("Anthropic URL failed: {}", e),
    }
}

#[tokio::test]
async fn test_gemini_base64() {
    let api_key = env_key("GEMINI_API_KEY");
    let b64 = read_image_base64("/Users/jieye/Downloads/IMG_0C360C6B7B8A-1.jpeg");

    println!("\n=== Gemini (gemini-2.0-flash) — raw base64 ===");
    match run_vl_test("gemini", api_key, "gemini-2.0-flash", &b64).await {
        Ok(text) => {
            println!("Response: {}", text);
            assert!(!text.is_empty(), "Gemini returned empty response");
        }
        Err(e) => panic!("Gemini failed: {}", e),
    }
}

#[tokio::test]
async fn test_qwen_base64() {
    let api_key = env_key("QWEN_API_KEY");
    let b64 = read_image_base64("/Users/jieye/Downloads/IMG_0C360C6B7B8A-1.jpeg");

    println!("\n=== Qwen (qwen-vl-plus) — raw base64 ===");
    match run_vl_test("qwen", api_key, "qwen-vl-plus", &b64).await {
        Ok(text) => {
            println!("Response: {}", text);
            assert!(!text.is_empty(), "Qwen returned empty response");
        }
        Err(e) => panic!("Qwen failed: {}", e),
    }
}

#[tokio::test]
async fn test_qwen_url() {
    let api_key = env_key("QWEN_API_KEY");
    let url = "https://upload.wikimedia.org/wikipedia/commons/thumb/4/47/PNG_transparency_demonstration_1.png/280px-PNG_transparency_demonstration_1.png";

    println!("\n=== Qwen (qwen-vl-plus) — URL ===");
    match run_vl_test("qwen", api_key, "qwen-vl-plus", url).await {
        Ok(text) => {
            println!("Response: {}", text);
            assert!(!text.is_empty(), "Qwen URL returned empty response");
        }
        Err(e) => panic!("Qwen URL failed: {}", e),
    }
}

/// Test vision validation: non-VL model with images should fail at build time
#[tokio::test]
async fn test_vision_validation_rejects_non_vl_model() {
    let dir = tempdir().unwrap();
    let workflows_dir = dir.path().join("workflows");
    std::fs::create_dir(&workflows_dir).unwrap();

    // deepseek-chat does NOT support vision
    write_workflow(&workflows_dir, "bad_vl", "deepseek", "deepseek-chat");

    let result = EngineBuilder::new()
        .workflows_dir(&workflows_dir)
        .provider("deepseek", "fake-key")
        .build()
        .await;

    assert!(result.is_err(), "Should reject non-VL model with images");
    let err = result.err().unwrap().to_string();
    println!("Validation error: {}", err);
    assert!(err.contains("does not support vision"), "Error should mention vision: {}", err);
}
