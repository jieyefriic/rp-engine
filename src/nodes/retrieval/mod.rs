mod keyword_search;
mod text_chunk;

pub(super) use keyword_search::KeywordSearch;
pub(super) use text_chunk::TextChunk;

use super::resolve_single_binding;
use crate::{EngineError, ExecutionState, YamlNode};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

// These nodes operate on request-sized in-memory corpora, not a persistent index.
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 100_000;

fn invalid(node: &YamlNode, message: impl std::fmt::Display) -> EngineError {
    EngineError::InvalidOperation(format!("Node '{}': {message}", node.id))
}

fn options<T: DeserializeOwned>(node: &YamlNode) -> Result<T, EngineError> {
    serde_json::from_value(json!(node.config.operation)).map_err(|e| invalid(node, e))
}

fn input(node: &YamlNode, state: &ExecutionState, key: &str) -> Result<Value, EngineError> {
    node.config
        .variables
        .get(key)
        .and_then(|b| resolve_single_binding(b, state))
        .ok_or_else(|| invalid(node, format!("missing variable '{key}'")))
}

#[derive(Deserialize)]
struct Document {
    id: String,
    text: String,
}
