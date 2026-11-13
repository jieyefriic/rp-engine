//! Reciprocal rank fusion across named, already-ranked retrieval sources.
use super::{resolve_single_binding, NodeExecutor};
use crate::{EngineError, ExecutionConfig, ExecutionState, YamlNode};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

pub struct RankFusionExecutor;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    #[serde(default = "default_id_field")]
    id_field: String,
    limit: Option<usize>,
}

fn default_id_field() -> String {
    "id".to_owned()
}

struct Hit {
    item: Value,
    score: f64,
    ranks: BTreeMap<String, usize>,
}

#[async_trait]
impl NodeExecutor for RankFusionExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        _: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let invalid =
            |msg: String| EngineError::InvalidOperation(format!("Node '{}': {msg}", node.id));
        let options: Options = serde_json::from_value(json!(node.config.operation))
            .map_err(|e| invalid(format!("invalid rank_fusion operation: {e}")))?;
        if options.limit == Some(0) {
            return Err(invalid("limit must be positive".into()));
        }
        if options.id_field.is_empty() || node.config.variables.is_empty() {
            return Err(invalid(
                "rank_fusion requires a nonempty id_field and at least one source in variables"
                    .into(),
            ));
        }

        // A stable traversal makes both representative selection and ties reproducible.
        let sources: BTreeMap<_, _> = node.config.variables.iter().collect();
        let mut positions: HashMap<Value, usize> = HashMap::new();
        let mut hits: Vec<Hit> = Vec::new();
        for (source, binding) in sources {
            let Some(Value::Array(items)) = resolve_single_binding(binding, state) else {
                return Err(invalid(format!(
                    "source '{source}' must resolve to a ranked array"
                )));
            };
            for (offset, item) in items.into_iter().enumerate() {
                let rank = offset + 1;
                let id = item.get(&options.id_field)
                    .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()) || v.is_u64() || v.is_i64())
                    .ok_or_else(|| invalid(format!("source '{source}' at rank {rank} requires a nonempty string or integer '{}'", options.id_field)))?;
                let pos = *positions.entry(id.clone()).or_insert_with(|| {
                    hits.push(Hit {
                        item,
                        score: 0.0,
                        ranks: BTreeMap::new(),
                    });
                    hits.len() - 1
                });
                let hit = &mut hits[pos];
                if !hit.ranks.contains_key(source) {
                    hit.ranks.insert(source.clone(), rank);
                    hit.score += 1.0 / (60.0 + rank as f64);
                }
            }
        }

        let total = hits.len();
        // Stable sort retains first-seen order on exact score ties.
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        let results = hits
            .into_iter()
            .take(options.limit.unwrap_or(total))
            .map(|hit| json!({"item":hit.item, "score":hit.score, "ranks":hit.ranks}))
            .collect();
        Ok(HashMap::from([
            ("results".into(), Value::Array(results)),
            ("total_count".into(), json!(total)),
        ]))
    }
}
