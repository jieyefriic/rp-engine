//! cargo run --offline --example local_retrieval [-- input.json]
use riceprompt_engine::{Engine, ExecutionStatus};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input: Value = if let Some(path) = std::env::args_os().nth(1) {
        serde_json::from_reader(std::fs::File::open(path)?)?
    } else {
        serde_json::from_str(include_str!("workflows/local_retrieval.input.json"))?
    };
    let result = Engine::builder()
        .build()
        .await?
        .run_yaml(include_str!("workflows/local_retrieval.yaml"), input)
        .await?;
    if result.status != ExecutionStatus::Success {
        return Err(format!("workflow {:?}: {:?}", result.status, result.error).into());
    }
    println!("{}", serde_json::to_string_pretty(&result.output)?);
    Ok(())
}
