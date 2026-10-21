//! Offline regression tests. No provider credentials or network access required.

use async_trait::async_trait;
use riceprompt_engine::{
    CheckpointState, CustomNode, Engine, EngineError, ExecutionConfig, ExecutionState,
    ExecutionStatus, NodeConfig,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

struct Probe {
    kind: &'static str,
    calls: Arc<AtomicUsize>,
    cancelled: Arc<AtomicUsize>,
}

struct DropProbe(Arc<AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl CustomNode for Probe {
    fn node_type(&self) -> &str {
        self.kind
    }

    async fn execute(
        &self,
        _: &NodeConfig,
        _: &ExecutionState,
        _: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.kind == "never" {
            let _guard = DropProbe(self.cancelled.clone());
            std::future::pending::<()>().await;
        }
        if self.kind == "retry_error" {
            return Err(EngineError::NodeExecutionFailed(
                "offline test error".into(),
            ));
        }
        Ok(HashMap::from([("value".into(), json!("saved"))]))
    }
}

fn probe(kind: &'static str) -> Arc<Probe> {
    Arc::new(Probe {
        kind,
        calls: Arc::new(AtomicUsize::new(0)),
        cancelled: Arc::new(AtomicUsize::new(0)),
    })
}

const PARALLEL: &str = r#"
version: "1.0"
name: safety
nodes:
  - {id: start, type: start}
  - {id: ask, type: checkpoint, config: {output: {question: start.question}}}
  - {id: write, type: counter}
  - id: response
    type: response
    config:
      variables: {saved: write.value, answer: start.answer}
      output: {saved: write.value, answer: start.answer}
edges:
  - {from: start, to: ask}
  - {from: start, to: write}
  - {from: ask, to: response}
"#;

#[tokio::test]
async fn deadline_cancels_pending_node_and_keeps_completed_sibling() {
    let never = probe("never");
    let counter = probe("counter");
    let yaml = PARALLEL.replace("type: checkpoint", "type: never");
    let engine = Engine::builder()
        .register_node(never.clone())
        .register_node(counter.clone())
        .timeout(Duration::from_millis(80))
        .build()
        .await
        .unwrap();
    let start = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(2), engine.run_yaml(&yaml, json!({})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.status, ExecutionStatus::Timeout);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(result.timing.total_ms >= 70);
    assert_eq!(
        result.executions["write"][0].outputs["value"],
        json!("saved")
    );
    assert!(!result.executions.contains_key("ask"));
    assert_eq!(never.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(counter.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn deadline_covers_retry_backoff() {
    let retry = probe("retry_error");
    let yaml = r#"
name: retry
nodes:
  - {id: start, type: start}
  - id: retry
    type: retry_error
    config:
      error_handling: {retry: 5, retry_delay_ms: 10000}
  - {id: response, type: response}
edges:
  - {from: start, to: retry}
  - {from: retry, to: response}
"#;
    let engine = Engine::builder()
        .register_node(retry.clone())
        .timeout(Duration::from_millis(80))
        .build()
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), engine.run_yaml(yaml, json!({})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.status, ExecutionStatus::Timeout);
    assert_eq!(retry.calls.load(Ordering::SeqCst), 1);
}

async fn checkpoint_roundtrip(legacy: bool) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("safety.yaml"), PARALLEL).unwrap();
    let counter = probe("counter");
    let engine = Engine::builder()
        .workflows_dir(dir.path())
        .register_node(counter.clone())
        .build()
        .await
        .unwrap();
    let result = engine
        .run("safety", json!({"question": "Continue?"}))
        .await
        .unwrap();
    assert_eq!(result.status, ExecutionStatus::Paused);
    assert_eq!(result.executions["write"].len(), 1);
    assert!(!result.executions.contains_key("response"));
    let mut encoded = serde_json::to_value(result.checkpoint_state.unwrap()).unwrap();
    if legacy {
        encoded
            .as_object_mut()
            .unwrap()
            .remove("pending_checkpoints");
    }
    let checkpoint: CheckpointState = serde_json::from_value(encoded).unwrap();
    let result = engine
        .resume("safety", checkpoint, "ask", json!({"answer": "yes"}))
        .await
        .unwrap();
    assert_eq!(
        result.status,
        ExecutionStatus::Success,
        "{:?}",
        result.error
    );
    assert_eq!(result.output["answer"], json!("yes"));
    assert_eq!(result.output["saved"], json!("saved"));
    assert_eq!(result.executions["ask"].len(), 1);
    assert_eq!(result.executions["write"].len(), 1);
    assert_eq!(counter.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn checkpoint_commits_batch_and_resume_does_not_repeat_side_effects() {
    checkpoint_roundtrip(false).await;
}

#[tokio::test]
async fn legacy_checkpoint_can_still_resume() {
    checkpoint_roundtrip(true).await;
}

#[tokio::test]
async fn parallel_checkpoints_require_each_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = PARALLEL
        .replace("type: counter", "type: checkpoint")
        .replace("saved: write.value", "saved: start.question");
    std::fs::write(dir.path().join("safety.yaml"), yaml).unwrap();
    let engine = Engine::builder()
        .workflows_dir(dir.path())
        .build()
        .await
        .unwrap();
    let paused = engine
        .run("safety", json!({"question": "Continue?"}))
        .await
        .unwrap();
    let checkpoint = paused.checkpoint_state.unwrap();
    assert_eq!(checkpoint.pending_checkpoints, vec!["ask", "write"]);
    assert!(engine
        .resume("safety", checkpoint.clone(), "missing", json!({}))
        .await
        .is_err());
    let paused = engine
        .resume("safety", checkpoint, "ask", json!({"answer": "yes"}))
        .await
        .unwrap();
    assert_eq!(paused.status, ExecutionStatus::Paused);
    assert!(!paused.executions.contains_key("response"));
    let checkpoint = paused.checkpoint_state.unwrap();
    assert_eq!(checkpoint.pending_checkpoints, vec!["write"]);
    assert!(engine
        .resume("safety", checkpoint.clone(), "ask", json!({}))
        .await
        .is_err());
    let done = engine
        .resume("safety", checkpoint, "write", json!({}))
        .await
        .unwrap();
    assert_eq!(done.status, ExecutionStatus::Success);
    assert_eq!(done.executions["ask"].len(), 1);
    assert_eq!(done.executions["write"].len(), 1);
}

#[tokio::test]
async fn acknowledged_checkpoint_routes_using_new_input() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = PARALLEL.replace(
        "{from: ask, to: response}",
        r#"{from: ask, to: response, condition: 'start.answer == "yes"'}"#,
    );
    std::fs::write(dir.path().join("safety.yaml"), yaml).unwrap();
    let engine = Engine::builder()
        .workflows_dir(dir.path())
        .register_node(probe("counter"))
        .build()
        .await
        .unwrap();
    let paused = engine.run("safety", json!({"answer": "no"})).await.unwrap();
    let done = engine
        .resume(
            "safety",
            paused.checkpoint_state.unwrap(),
            "ask",
            json!({"answer": "yes"}),
        )
        .await
        .unwrap();
    assert_eq!(done.status, ExecutionStatus::Success, "{:?}", done.error);
    assert_eq!(done.output["answer"], json!("yes"));
}

#[tokio::test]
async fn batch_edge_conditions_see_completed_siblings_before_pause() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = PARALLEL
        .replace(
            "  - {id: write, type: counter}",
            "  - {id: a_route, type: counter}\n  - {id: write, type: counter}",
        )
        .replace(
            "  - {from: start, to: write}",
            "  - {from: start, to: a_route}\n  - {from: start, to: write}",
        )
        .replace(
            "{from: ask, to: response}",
            r#"{from: a_route, to: response, condition: 'write.value == "saved"'}"#,
        );
    std::fs::write(dir.path().join("safety.yaml"), yaml).unwrap();
    let engine = Engine::builder()
        .workflows_dir(dir.path())
        .register_node(probe("counter"))
        .build()
        .await
        .unwrap();
    let paused = engine.run("safety", json!({})).await.unwrap();
    let done = engine
        .resume(
            "safety",
            paused.checkpoint_state.unwrap(),
            "ask",
            json!({"answer": "yes"}),
        )
        .await
        .unwrap();
    assert_eq!(done.status, ExecutionStatus::Success, "{:?}", done.error);
    assert_eq!(done.output["saved"], json!("saved"));
}

#[tokio::test]
async fn sequential_checkpoints_pause_at_each_new_checkpoint_once() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = PARALLEL
        .replace("type: counter", "type: checkpoint")
        .replace("{from: start, to: write}", "{from: ask, to: write}")
        .replace("{from: ask, to: response}", "{from: write, to: response}")
        .replace("saved: write.value", "saved: start.question");
    std::fs::write(dir.path().join("safety.yaml"), yaml).unwrap();
    let engine = Engine::builder()
        .workflows_dir(dir.path())
        .build()
        .await
        .unwrap();
    let first = engine
        .run("safety", json!({"question": "Continue?"}))
        .await
        .unwrap();
    assert!(!first.executions.contains_key("write"));
    let second = engine
        .resume(
            "safety",
            first.checkpoint_state.unwrap(),
            "ask",
            json!({"answer": "yes"}),
        )
        .await
        .unwrap();
    assert_eq!(second.status, ExecutionStatus::Paused);
    assert_eq!(
        second
            .checkpoint_state
            .as_ref()
            .unwrap()
            .pending_checkpoints,
        vec!["write"]
    );
    let done = engine
        .resume(
            "safety",
            second.checkpoint_state.unwrap(),
            "write",
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(done.status, ExecutionStatus::Success);
    assert_eq!(done.executions["ask"].len(), 1);
    assert_eq!(done.executions["write"].len(), 1);
}

#[tokio::test]
async fn response_completed_in_paused_batch_is_preserved_after_resume() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = r#"
name: response
nodes:
  - {id: start, type: start}
  - {id: ask, type: checkpoint}
  - {id: response, type: response, config: {output: {value: start.value}}}
edges:
  - {from: start, to: ask}
  - {from: start, to: response}
"#;
    std::fs::write(dir.path().join("response.yaml"), yaml).unwrap();
    let engine = Engine::builder()
        .workflows_dir(dir.path())
        .build()
        .await
        .unwrap();
    let paused = engine
        .run("response", json!({"value": "saved"}))
        .await
        .unwrap();
    assert!(paused.executions.contains_key("response"));
    let done = engine
        .resume(
            "response",
            paused.checkpoint_state.unwrap(),
            "ask",
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(done.status, ExecutionStatus::Success);
    assert_eq!(done.output["value"], json!("saved"));
    assert_eq!(done.executions["response"].len(), 1);
}

const SNAPSHOT: &str = r#"
name: snapshot
providers:
  openai:
    api_key: fake-inline-secret
nodes:
  - {id: start, type: start}
  - {id: response, type: response, config: {output: {message: start.message}}}
edges:
  - {from: start, to: response}
"#;

#[tokio::test]
async fn snapshot_is_redacted_by_default_with_explicit_raw_and_disabled_modes() {
    let engine = Engine::builder().build().await.unwrap();
    let result = engine
        .run_yaml(SNAPSHOT, json!({"message": "ok"}))
        .await
        .unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
    assert_eq!(result.output["message"], json!("ok"));
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("fake-inline-secret"));
    let snapshot: serde_yaml::Value =
        serde_yaml::from_str(result.source_yaml.as_ref().unwrap()).unwrap();
    assert_eq!(
        snapshot["providers"]["openai"]["api_key"].as_str(),
        Some("[REDACTED]")
    );
    assert_eq!(snapshot["nodes"].as_sequence().unwrap().len(), 2);

    let raw = Engine::builder()
        .capture_source_yaml_raw(true)
        .build()
        .await
        .unwrap();
    assert_eq!(
        raw.run_yaml(SNAPSHOT, json!({}))
            .await
            .unwrap()
            .source_yaml
            .as_deref(),
        Some(SNAPSHOT)
    );
    let disabled = Engine::builder()
        .capture_source_yaml_raw(true)
        .capture_source_yaml(false)
        .build()
        .await
        .unwrap();
    assert!(disabled
        .run_yaml(SNAPSHOT, json!({}))
        .await
        .unwrap()
        .source_yaml
        .is_none());
}
