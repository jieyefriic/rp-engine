use super::{input, invalid, options, Document, MAX_BYTES, MAX_ITEMS};
use crate::{nodes::NodeExecutor, EngineError, ExecutionConfig, ExecutionState, YamlNode};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub(crate) struct TextChunk;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Options {
    max_chars: usize,
    overlap_chars: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_chars: 512,
            overlap_chars: 0,
        }
    }
}

// Return byte offsets just after each paragraph break. Recognize LF and CRLF.
fn paragraph_ends(text: &str) -> impl Iterator<Item = usize> + '_ {
    let mut offset = 0;
    text.split_inclusive('\n').filter_map(move |line| {
        offset += line.len();
        (line.ends_with('\n') && line.trim().is_empty()).then_some(offset)
    })
}

#[async_trait]
impl NodeExecutor for TextChunk {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        _: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let opts: Options = options(node)?;
        if opts.max_chars == 0 || opts.overlap_chars >= opts.max_chars {
            return Err(invalid(node, "require 0 <= overlap_chars < max_chars"));
        }
        let Value::Array(documents) = input(node, state, "documents")? else {
            return Err(invalid(node, "documents must be an array"));
        };
        if documents.len() > MAX_ITEMS {
            return Err(invalid(node, "too many documents"));
        }
        let mut chunks = Vec::new();
        let mut ids = HashSet::new();
        let (mut input_bytes, mut output_bytes) = (0usize, 0usize);
        for (index, doc) in documents.into_iter().enumerate() {
            let Document { id, text } = serde_json::from_value(doc)
                .map_err(|e| invalid(node, format!("documents[{index}]: {e}")))?;
            if id.is_empty() || !ids.insert(id.clone()) {
                return Err(invalid(
                    node,
                    format!("documents[{index}] needs a unique, nonempty string id"),
                ));
            }
            input_bytes += id.len() + text.len();
            if input_bytes > MAX_BYTES {
                return Err(invalid(node, "document input exceeds 16 MiB"));
            }
            // Convert character windows to UTF-8 offsets without rescanning a prefix on every step.
            let offsets: Vec<_> = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .collect();
            let n = offsets.len() - 1;
            let breaks: Vec<_> = paragraph_ends(&text).collect();
            let mut first = 0;
            while first < n {
                let cap = first.saturating_add(opts.max_chars).min(n);
                let end = if cap < n {
                    let count = breaks.partition_point(|&p| p <= offsets[cap]);
                    count
                        .checked_sub(1)
                        .map(|i| breaks[i])
                        .filter(|&p| p > offsets[first + opts.overlap_chars])
                        .map(|p| offsets.partition_point(|&b| b < p))
                        .unwrap_or(cap)
                } else {
                    cap
                };
                let (lo, hi) = (offsets[first], offsets[end]);
                output_bytes += hi - lo + 2 * id.len();
                if chunks.len() == MAX_ITEMS || output_bytes > MAX_BYTES {
                    return Err(invalid(node, "chunk output exceeds the item or byte budget; reduce overlap or split the batch"));
                }
                chunks.push(json!({"id":format!("{id}@{lo}:{hi}"), "doc_id":id,
                    "start_byte":lo, "end_byte":hi, "text":&text[lo..hi]}));
                if end == n {
                    break;
                }
                first = end - opts.overlap_chars;
            }
        }
        Ok(HashMap::from([
            ("count".into(), json!(chunks.len())),
            ("chunks".into(), Value::Array(chunks)),
        ]))
    }
}
