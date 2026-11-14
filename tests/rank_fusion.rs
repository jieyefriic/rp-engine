//! Runs the documented workflow without provider keys or network access.
use riceprompt_engine::{Engine, ExecutionStatus};
use serde_json::{json, Value};

const FLOW: &str = include_str!("../examples/workflows/rank_fusion.yaml");

async fn run(
    flow: &str,
    input: Value,
) -> Result<riceprompt_engine::ExecutionResult, riceprompt_engine::EngineError> {
    Engine::builder()
        .build()
        .await
        .unwrap()
        .run_yaml(flow, input)
        .await
}

#[tokio::test]
async fn fuses_ranked_lists_and_preserves_provenance() {
    let result = run(
        FLOW,
        json!({
            "keyword": [{"id":"a"}, {"id":"b","text":"lexical"}, {"id":"a"}, {"id":"c"}],
            "vector": [{"id":"b","text":"semantic"}, {"id":"d"}]
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        result.status,
        ExecutionStatus::Success,
        "{:?}",
        result.error
    );
    let hits = result.output["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(result.output["total"], 4);
    assert_eq!(hits[0]["item"], json!({"id":"b","text":"lexical"}));
    assert_eq!(hits[0]["ranks"], json!({"keyword":2,"vector":1}));
    assert!((hits[0]["score"].as_f64().unwrap() - (1.0 / 62.0 + 1.0 / 61.0_f64)).abs() < 1e-12);
    assert_eq!(hits[1]["item"]["id"], "a");
    assert_eq!(hits[1]["ranks"], json!({"keyword":1}));
    assert_eq!(hits[2]["item"]["id"], "d");
}

#[tokio::test]
async fn ties_are_stable_and_numeric_and_string_ids_are_distinct() {
    let flow = FLOW.replace("id_field: id, limit: 3", "id_field: doc");
    let result = run(
        &flow,
        json!({"keyword":[{"doc":7}], "vector":[{"doc":"7"}]}),
    )
    .await
    .unwrap();
    assert_eq!(result.output["total"], 2);
    let hits = &result.output["hits"];
    assert_eq!(hits[0]["item"]["doc"], 7);
    assert_eq!(hits[1]["item"]["doc"], "7");
    let empty = run(&flow, json!({"keyword":[], "vector":[]}))
        .await
        .unwrap();
    assert_eq!(json!(empty.output), json!({"hits":[],"total":0}));
}

#[tokio::test]
async fn invalid_sources_and_options_fail_explicitly() {
    for (flow, keyword, message) in [
        (FLOW.to_string(), json!({"id":1}), "keyword"),
        (FLOW.to_string(), json!([{}]), "id"),
        (FLOW.to_string(), json!([{"id":true}]), "id"),
        (FLOW.replace("limit: 3", "limit: 0"), json!([]), "limit"),
    ] {
        let error = run(&flow, json!({"keyword":keyword, "vector":[]}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
}
