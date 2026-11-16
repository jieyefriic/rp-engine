//! Offline end-to-end contract: exercise the example's actual graph.
use riceprompt_engine::{Engine, ExecutionStatus};
use serde_json::{json, Value};
const FLOW: &str = include_str!("../examples/inventory_diff/workflow.yaml");
const BEFORE: &str = include_str!("../examples/inventory_diff/before.csv");
const AFTER: &str = include_str!("../examples/inventory_diff/after.csv");
const HEADER: &str = "site,sku,quantity,description\n";

async fn run(flow: &str, left: &str, right: &str) -> Result<Value, String> {
    let result = Engine::builder()
        .build()
        .await
        .unwrap()
        .run_yaml(flow, json!({"before_csv":left,"after_csv":right}))
        .await
        .map_err(|e| e.to_string())?;
    assert_eq!(
        result.status,
        ExecutionStatus::Success,
        "{:?}",
        result.error
    );
    Ok(result.output["report"].clone())
}

#[tokio::test]
async fn two_exports_become_a_source_linked_inventory_report() {
    let report = run(FLOW, BEFORE, AFTER).await.unwrap();
    assert_eq!(
        report,
        json!({
            "summary":{"added":1,"removed":1,"changed":1,"conflicts":1,"unchanged":2},
            "added":[{"key":{"site":"West","sku":"005"},"record":6,"values":{"quantity":"3","description":"Clip \"S\""}}],
            "removed":[{"key":{"site":"East","sku":"002"},"record":3,"values":{"quantity":"5","description":"Washer"}}],
            "changed":[{"key":{"site":"East","sku":"001"},"before_record":2,"after_record":4,"fields":{"quantity":{"before":"12","after":"9"}}}],
            "conflicts":[{"key":{"site":"West","sku":"003"},"before_records":[5,6],"after_records":[2]}]
        })
    );
}

#[tokio::test]
async fn quoting_unicode_and_logical_record_numbers_survive_reordered_exports() {
    let before =
        "\u{feff}site,sku,quantity,description\r\n东,001,2,\"line 1\r\nline 2, \"\"分隔\"\"\"\r\n";
    let before = format!("{before}东,002,01,尾行");
    let after =
        "sku,site,quantity,description\n002,东,1,尾行\n001,东,2,\"line 1\r\nline 2, \"\"分隔\"\"\"";
    let report = run(FLOW, &before, after).await.unwrap();
    assert_eq!(
        report["summary"],
        json!({"unchanged":1,"changed":1,"added":0,"removed":0,"conflicts":0})
    );
    assert_eq!(report["changed"][0]["before_record"], 3);
    assert_eq!(
        report["changed"][0]["fields"],
        json!({"quantity":{"before":"01","after":"1"}})
    );
}

#[tokio::test]
async fn duplicate_keys_are_not_resolved_by_guessing_even_on_only_one_side() {
    let after = format!("{HEADER}A,X,1,one\nA,X,1,one\nA,Y,1,two\nA,Y,2,two\n");
    let before = format!("{HEADER}A,Y,1,two\nA,Y,2,two\n");
    let r = run(FLOW, &before, &after).await.unwrap();
    assert_eq!(
        r["summary"],
        json!({"conflicts":2,"added":0,"removed":0,"changed":0,"unchanged":0})
    );
    assert_eq!(r["conflicts"][0]["before_records"], json!([]));
    assert_eq!(r["conflicts"][0]["after_records"], json!([2, 3]));
    assert_eq!(r["conflicts"][1]["before_records"], json!([2, 3]));
    assert_eq!(r["conflicts"][1]["after_records"], json!([4, 5]));
}

#[tokio::test]
async fn header_only_and_composite_keys_are_unambiguous_and_sorted() {
    let empty = run(FLOW, HEADER, HEADER).await.unwrap();
    assert_eq!(
        empty["summary"],
        json!({"conflicts":0,"added":0,"removed":0,"changed":0,"unchanged":0})
    );
    let after = format!("{HEADER}a|b,c,1,x\na,b|c,2,y\n");
    let r = run(FLOW, HEADER, &after).await.unwrap();
    assert_eq!(r["added"].as_array().unwrap().len(), 2);
    assert_eq!(r["added"][0]["key"], json!({"site":"a","sku":"b|c"}));
    assert_eq!(r["added"][1]["key"], json!({"site":"a|b","sku":"c"}));
}

#[tokio::test]
async fn invalid_exports_and_missing_columns_never_become_empty_diffs() {
    for (text, message) in [
        ("site,site,quantity,description\n", "unique"),
        ("site,sku,quantity,description\nA,1,4\n", "record 2"),
        (
            "site,sku,quantity,description\nA,1,4,\"oops",
            "unterminated",
        ),
        (
            "site,sku,quantity,description\nA,1,4,\"closed\"tail",
            "invalid quoting",
        ),
        ("site,sku,quantity,description\nA, ,4,ok", "blank key"),
        ("site,sku,description\n", "missing configured column"),
        ("site,,description\n", "nonblank"),
    ] {
        let err = run(FLOW, text, HEADER).await.unwrap_err();
        assert!(err.contains(message), "{err}");
    }
    for compare in ["[quantity, quantity]", "[site]", "[]"] {
        let err = run(
            &FLOW.replace("[quantity, description]", compare),
            HEADER,
            HEADER,
        )
        .await
        .unwrap_err();
        assert!(err.contains("distinct column lists"), "{err}");
    }
}

#[tokio::test]
async fn input_limits_fail_closed() {
    let large = format!("{HEADER}A,1,1,{}", "x".repeat(2 * 1024 * 1024));
    assert!(run(FLOW, &large, HEADER)
        .await
        .unwrap_err()
        .contains("2 MiB"));
    let many = format!("{HEADER}{}", "A,1,1,x\n".repeat(10_001));
    assert!(run(FLOW, &many, HEADER)
        .await
        .unwrap_err()
        .contains("row/cell limit"));
}

#[tokio::test]
async fn existing_custom_csv_node_still_wins_over_the_new_default() {
    use riceprompt_engine::{CustomNode, EngineError, ExecutionConfig, ExecutionState, NodeConfig};
    use std::{collections::HashMap, sync::Arc};

    struct ExistingCsv;
    #[async_trait::async_trait]
    impl CustomNode for ExistingCsv {
        fn node_type(&self) -> &str {
            "csv_table"
        }
        async fn execute(
            &self,
            _: &NodeConfig,
            _: &ExecutionState,
            _: &ExecutionConfig,
        ) -> Result<HashMap<String, Value>, EngineError> {
            Ok(HashMap::from([("marker".to_owned(), json!("existing"))]))
        }
    }
    let flow = r#"
name: prior_custom
nodes:
  - {id: start, type: start}
  - {id: parser, type: csv_table}
  - {id: response, type: response, config: {output: {marker: parser.marker}}}
edges: [{from: start, to: parser}, {from: parser, to: response}]
"#;
    let result = Engine::builder()
        .register_node(Arc::new(ExistingCsv))
        .build()
        .await
        .unwrap()
        .run_yaml(flow, json!({}))
        .await
        .unwrap();
    assert_eq!(result.output["marker"], "existing");
}

#[tokio::test]
async fn accepts_a_table_from_another_node_without_coercing_invalid_cells() {
    let flow = r#"
name: direct_tables
nodes:
  - {id: start, type: start}
  - id: diff
    type: reconcile_tables
    config:
      variables: {before: start.before, after: start.after}
      operation: {keys: [id], compare: [qty]}
  - {id: response, type: response, config: {output: {report: diff.report}}}
edges: [{from: start, to: diff}, {from: diff, to: response}]
"#;
    let engine = Engine::builder().build().await.unwrap();
    let before = json!({"columns":["id","qty"],"rows":[["001","1"]]});
    let after = json!({"columns":["qty","id"],"rows":[["01","001"]]});
    let result = engine
        .run_yaml(flow, json!({"before":before,"after":after}))
        .await
        .unwrap();
    assert_eq!(
        result.output["report"]["changed"][0]["fields"]["qty"],
        json!({"before":"1","after":"01"})
    );
    let bad = json!({"columns":["id","qty"],"rows":[["001",1]]});
    let err = engine
        .run_yaml(flow, json!({"before":before,"after":bad}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("after: expected a table"));
}
