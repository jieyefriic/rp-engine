//! cargo run --example event_sessions -- [events.jsonl]
use riceprompt_engine::{Engine, ExecutionStatus};
use serde_json::json;
use std::{error::Error, io::Read};
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let text = match args.as_slice() {
        [] => include_str!("event_sessions/events.jsonl").to_owned(),
        [path] => {
            let mut s = String::new();
            std::fs::File::open(path)?
                .take(2 * 1024 * 1024 + 1)
                .read_to_string(&mut s)?;
            s
        }
        _ => return Err("usage: event_sessions [events.jsonl]".into()),
    };
    let r = Engine::builder()
        .build()
        .await?
        .run_yaml(
            include_str!("event_sessions/workflow.yaml"),
            json!({"log":text}),
        )
        .await?;
    if r.status != ExecutionStatus::Success {
        return Err(format!("{:?}", r.status).into());
    }
    println!("{}", serde_json::to_string_pretty(&r.output["report"])?);
    Ok(())
}
