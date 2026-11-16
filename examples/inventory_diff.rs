//! Run without arguments for a bundled example, or pass two UTF-8 inventory CSV files.
use riceprompt_engine::{Engine, ExecutionStatus};
use serde_json::json;
use std::{env, error::Error, fs::File, io::Read, path::Path};
const WORKFLOW: &str = include_str!("inventory_diff/workflow.yaml");

fn read_csv(path: &Path) -> Result<String, Box<dyn Error>> {
    let mut text = String::new();
    File::open(path)?
        .take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut text)?;
    if text.len() > 2 * 1024 * 1024 {
        return Err("CSV exceeds 2 MiB".into());
    }
    Ok(text)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let (before, after) = match &args[..] {
        [] => (
            include_str!("inventory_diff/before.csv").into(),
            include_str!("inventory_diff/after.csv").into(),
        ),
        [l, r] => (read_csv(Path::new(l))?, read_csv(Path::new(r))?),
        _ => return Err("usage: inventory_diff [before.csv after.csv]".into()),
    };
    let result = Engine::builder()
        .build()
        .await?
        .run_yaml(WORKFLOW, json!({"before_csv":before,"after_csv":after}))
        .await?;
    if result.status != ExecutionStatus::Success {
        return Err(format!("workflow {:?}: {:?}", result.status, result.error).into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&result.output["report"])?
    );
    Ok(())
}
