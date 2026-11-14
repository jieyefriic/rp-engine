use riceprompt_engine::{Engine, EngineError, ExecutionResult, ExecutionStatus};
use serde_json::{json, Value};

const FLOW: &str = include_str!("../examples/workflows/local_retrieval.yaml");
fn sample() -> Value {
    serde_json::from_str(include_str!(
        "../examples/workflows/local_retrieval.input.json"
    ))
    .unwrap()
}
async fn run(flow: &str, input: Value) -> Result<ExecutionResult, EngineError> {
    Engine::builder()
        .build()
        .await
        .unwrap()
        .run_yaml(flow, input)
        .await
}

#[tokio::test]
async fn chinese_question_returns_a_verbatim_citation_without_a_vector_source() {
    let input = sample();
    let out = run(FLOW, input.clone()).await.unwrap();
    assert_eq!(out.status, ExecutionStatus::Success, "{:?}", out.error);
    let top = &out.output["passages"][0];
    assert_eq!(top["ranks"], json!({"keyword":1}));
    let item = &top["item"];
    assert_eq!(item["doc_id"], "library");
    let text = input["documents"][0]["text"].as_str().unwrap();
    let start = item["start_byte"].as_u64().unwrap() as usize;
    let end = item["end_byte"].as_u64().unwrap() as usize;
    assert!(start > 0);
    assert_eq!(item["text"], &text[start..end]);
    assert!(text[start..end].contains("东门的自助还书箱"));
}

#[tokio::test]
async fn english_query_and_optional_vector_ranking_agree_on_a_stable_chunk_id() {
    let mut input = sample();
    input["query"] = json!("IRRIGATION pump");
    let first = run(FLOW, input.clone()).await.unwrap();
    let hit = &first.output["passages"][0]["item"];
    assert_eq!(hit["doc_id"], "garden");
    input["vector"] = json!([{"id":hit["id"]}]);
    let second = run(FLOW, input).await.unwrap();
    let fused = &second.output["passages"][0];
    assert_eq!(fused["ranks"], json!({"keyword":1,"vector":1}));
    assert_eq!(fused["item"], *hit);
}

#[tokio::test]
async fn chunk_offsets_cover_original_with_exact_overlap_and_paragraph_preference() {
    let text = "甲🪴\r\n\r\n乙abc你好末";
    let flow = FLOW.replace("48, overlap_chars: 4", "8, overlap_chars: 2");
    let result = run(
        &flow,
        json!({"documents":[{"id":"p@:", "text":text}], "query":"乙"}),
    )
    .await
    .unwrap();
    let chunks = result.executions["split"][0].outputs["chunks"]
        .as_array()
        .unwrap();
    assert_eq!(chunks[0]["text"], "甲🪴\r\n\r\n");
    let mut last_end = 0;
    for (i, c) in chunks.iter().enumerate() {
        let (lo, hi) = (
            c["start_byte"].as_u64().unwrap() as usize,
            c["end_byte"].as_u64().unwrap() as usize,
        );
        assert_eq!(c["text"], &text[lo..hi]);
        assert!(text[lo..hi].chars().count() <= 8);
        if i > 0 {
            assert_eq!(text[lo..last_end].chars().count(), 2);
        }
        assert!(hi > last_end);
        last_end = hi;
    }
    assert_eq!(last_end, text.len());
}

#[tokio::test]
async fn bm25_uses_term_frequency_and_length_and_stable_ties() {
    let flow = FLOW.replace("limit: 10", "limit: 2");
    let docs = json!([
        {"id":"long","text":"tea water leaf field"},
        {"id":"short-a","text":"tea tea"},
        {"id":"short-b","text":"tea tea"},
        {"id":"none","text":"rain"}
    ]);
    let result = run(&flow, json!({"documents":docs,"query":"TEA tea"}))
        .await
        .unwrap();
    let out = &result.executions["find"][0].outputs;
    assert_eq!(out["total_count"], 3);
    let hits = out["results"].as_array().unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0]["doc_id"], "short-a");
    assert_eq!(hits[1]["doc_id"], "short-b");
    let idf = (1.0_f64 + 1.5 / 3.5).ln();
    let score = idf * 2.0 * 2.2 / (2.0 + 1.2 * (0.25 + 0.75 * 2.0 / 2.25));
    assert!((hits[0]["bm25_score"].as_f64().unwrap() - score).abs() < 1e-12);
}

#[tokio::test]
async fn blank_no_match_and_empty_corpus_have_no_hits() {
    for (docs, query) in [
        (json!([]), "茶"),
        (json!([{"id":"a","text":""}]), "tea"),
        (sample()["documents"].clone(), "!!!"),
        (sample()["documents"].clone(), "unobtainium"),
    ] {
        let result = run(FLOW, json!({"documents":docs,"query":query}))
            .await
            .unwrap();
        assert_eq!(result.output["passages"], json!([]));
    }
}

#[tokio::test]
async fn invalid_inputs_and_overlapping_windows_fail_clearly() {
    for (flow, input, msg) in [
        (
            FLOW.into(),
            json!({"documents":[{"id":"a","text":""},{"id":"a","text":"x"}],"query":"x"}),
            "unique",
        ),
        (
            FLOW.into(),
            json!({"documents":[{"id":1,"text":"x"}],"query":"x"}),
            "documents[0]",
        ),
        (
            FLOW.into(),
            json!({"documents":[],"query":["tea"]}),
            "query",
        ),
        (
            FLOW.replace("overlap_chars: 4", "overlap_chars: 48"),
            sample(),
            "overlap_chars",
        ),
        (FLOW.replace("limit: 10", "limit: 0"), sample(), "limit"),
    ] {
        let err = run(&flow, input).await.unwrap_err().to_string();
        assert!(err.contains(msg), "{err}");
    }
}
