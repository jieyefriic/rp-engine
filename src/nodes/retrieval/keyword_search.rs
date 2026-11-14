use super::{input, invalid, options, MAX_BYTES, MAX_ITEMS};
use crate::{nodes::NodeExecutor, EngineError, ExecutionConfig, ExecutionState, YamlNode};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(crate) struct KeywordSearch;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Options {
    limit: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self { limit: 10 }
    }
}

fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF |
        0x20000..=0x2EE5F | 0x2F800..=0x2FA1F | 0x30000..=0x323AF)
}

// Unigrams permit single-character queries; adjacent Han pairs capture local word order.
// Borrow the reusable buffers: retain statistics only for terms present in the query.
fn tokens(text: &str, mut emit: impl FnMut(&str)) {
    let mut word = String::new();
    let mut previous = None;
    for c in text.chars() {
        if !is_han(c) && c.is_alphanumeric() {
            word.extend(c.to_lowercase());
            previous = None;
        } else {
            if !word.is_empty() {
                emit(&word);
                word.clear();
            }
            if is_han(c) {
                let mut bytes = [0; 8];
                let current = c.encode_utf8(&mut bytes);
                emit(current);
                if let Some(p) = previous {
                    let len = char::encode_utf8(p, &mut bytes).len();
                    let rest = c.encode_utf8(&mut bytes[len..]).len();
                    emit(
                        std::str::from_utf8(&bytes[..len + rest]).expect("two encoded characters"),
                    );
                }
                previous = Some(c);
            } else {
                previous = None;
            }
        }
    }
    if !word.is_empty() {
        emit(&word);
    }
}

#[async_trait]
impl NodeExecutor for KeywordSearch {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        _: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let opts: Options = options(node)?;
        if opts.limit == 0 {
            return Err(invalid(node, "limit must be positive"));
        }
        let Value::String(query) = input(node, state, "query")? else {
            return Err(invalid(node, "query must be a string"));
        };
        let Value::Array(items) = input(node, state, "items")? else {
            return Err(invalid(node, "items must be an array"));
        };
        if items.len() > MAX_ITEMS || query.len() > 8192 {
            return Err(invalid(node, "too many items or query exceeds 8 KiB"));
        }

        let mut terms = BTreeSet::new();
        tokens(&query, |term| {
            terms.insert(term.to_owned());
        });
        let n = items.len() as f64;
        let mut df: BTreeMap<&str, usize> = BTreeMap::new();
        let mut lengths = 0usize;
        let mut bytes = 0usize;
        let mut candidates = Vec::new();
        for (i, item) in items.into_iter().enumerate() {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid(node, format!("items[{i}] requires a text string")))?;
            if !item.get("id").is_some_and(|id| {
                id.as_str().is_some_and(|s| !s.is_empty()) || id.is_i64() || id.is_u64()
            }) {
                return Err(invalid(
                    node,
                    format!("items[{i}] requires a string or integer id"),
                ));
            }
            bytes += text.len();
            if bytes > MAX_BYTES {
                return Err(invalid(node, "search text exceeds 16 MiB"));
            }
            let mut tf: BTreeMap<&str, usize> = BTreeMap::new();
            let mut length = 0;
            tokens(text, |term| {
                length += 1;
                if let Some(key) = terms.get(term) {
                    *tf.entry(key).or_default() += 1;
                }
            });
            lengths += length;
            if !tf.is_empty() {
                for &key in tf.keys() {
                    *df.entry(key).or_default() += 1;
                }
                candidates.push((item, tf, length));
            }
        }

        const K1: f64 = 1.2;
        const B: f64 = 0.75;
        let avg_len = lengths as f64 / n.max(1.0);
        let mut hits: Vec<_> = candidates
            .into_iter()
            .map(|(item, tf, len)| {
                let norm = K1 * (1.0 - B + B * len as f64 / avg_len);
                let score: f64 = tf
                    .iter()
                    .map(|(term, &freq)| {
                        let docs = df[term] as f64;
                        let idf = ((n - docs + 0.5) / (docs + 0.5)).ln_1p();
                        idf * (freq as f64 * (K1 + 1.0)) / (freq as f64 + norm)
                    })
                    .sum();
                (score, item)
            })
            .collect();
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
        let total = hits.len();
        hits.truncate(opts.limit);
        let results = hits
            .into_iter()
            .map(|(score, mut item)| {
                item["bm25_score"] = json!(score);
                item
            })
            .collect();
        Ok(HashMap::from([
            ("total_count".into(), json!(total)),
            ("results".into(), Value::Array(results)),
        ]))
    }
}
