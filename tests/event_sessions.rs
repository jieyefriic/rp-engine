use riceprompt_engine::{Engine, ExecutionStatus};
use serde_json::{json, Value};
const FLOW: &str = include_str!("../examples/event_sessions/workflow.yaml");
async fn run(text: &str) -> Result<Value, String> {
    let r = Engine::builder()
        .build()
        .await
        .unwrap()
        .run_yaml(FLOW, json!({"log":text}))
        .await
        .map_err(|e| e.to_string())?;
    assert_eq!(r.status, ExecutionStatus::Success);
    Ok(r.output["report"].clone())
}
#[tokio::test]
async fn raw_log_produces_traceable_sessions() {
    let r = run(include_str!("../examples/event_sessions/events.jsonl"))
        .await
        .unwrap();
    assert_eq!(r["event_count"], 4);
    assert_eq!(r["entity_count"], 2);
    assert_eq!(r["session_count"], 3);
    let a = &r["sessions"][0];
    assert_eq!(a["entity"], "visitor-a");
    assert_eq!(a["duration_ms"], 120000);
    assert_eq!(a["events"][0]["action"], "browse");
    assert_eq!(a["events"][0]["source_line"], 4);
    assert_eq!(a["events"][1]["source_line"], 1);
    assert_eq!(r["sessions"][1]["events"][0]["action"], "return");
    assert_eq!(r["sessions"][2]["entity"], "visitor-b");
}
#[tokio::test]
async fn exact_idle_boundary_and_timestamp_ties() {
    let text=(0..3).map(|i|json!({"entity":"a","timestamp":if i==0{"2026-01-01T00:00:00Z"}else{"2026-01-01T00:05:00Z"},"action":i.to_string()}).to_string()).collect::<Vec<_>>().join("\n");
    let r = run(&text).await.unwrap();
    assert_eq!(r["session_count"], 1);
    assert_eq!(r["sessions"][0]["duration_ms"], 300000);
    assert_eq!(r["sessions"][0]["events"][2]["source_line"], 3);
}
#[tokio::test]
async fn bad_records_fail_with_source_line() {
    for text in [
        "{}",
        "[]",
        "not json",
        r#"{"entity":"a","timestamp":"yesterday","action":"b"}"#,
        r#"{"entity":"","timestamp":"2026-01-01T00:00:00Z","action":"b"}"#,
    ] {
        assert!(run(text).await.is_err());
    }
    assert!(run("\nnot json").await.unwrap_err().contains("line 2"));
}
#[tokio::test]
async fn empty_log_and_limits() {
    assert_eq!(run("\n ").await.unwrap()["session_count"], 0);
    assert!(run(&"x".repeat(2 * 1024 * 1024 + 1))
        .await
        .unwrap_err()
        .contains("2 MiB"));
    let e =
        json!({"entity":"a","timestamp":"2026-01-01T00:00:00Z","action":"b"}).to_string() + "\n";
    assert!(run(&e.repeat(10001)).await.unwrap_err().contains("10000"));
}
#[tokio::test]
async fn invalid_gap_and_extra_fields_are_rejected() {
    for gap in ["0", "86401", "-1", "1.5"] {
        let f = FLOW.replace("300", gap);
        assert!(Engine::builder()
            .build()
            .await
            .unwrap()
            .run_yaml(&f, json!({"log":""}))
            .await
            .is_err());
    }
    assert!(run(
        r#"{"entity":"a","timestamp":"2026-01-01T00:00:00Z","action":"b","unexpected":1}"#
    )
    .await
    .is_err());
}
