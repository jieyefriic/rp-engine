//! Offline JSONL event normalization and inactivity-based session reconstruction.
use super::{resolve_variables, NodeExecutor};
use crate::{EngineError, ExecutionConfig, ExecutionState, YamlNode};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
fn fail(s: impl Into<String>) -> EngineError {
    EngineError::NodeExecutionFailed(s.into())
}
const LIMIT: usize = 10_000;
#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
struct Event {
    entity: String,
    timestamp: String,
    action: String,
    source_line: usize,
}
impl Event {
    fn time(&self) -> Result<i64, EngineError> {
        if self.entity.trim().is_empty()
            || self.action.trim().is_empty()
            || self.entity.len() > 256
            || self.action.len() > 256
            || self.timestamp.len() > 64
            || self.source_line == 0
        {
            return Err(fail(
                "event: invalid entity, action, timestamp length or source line",
            ));
        }
        chrono::DateTime::parse_from_rfc3339(&self.timestamp)
            .map(|t| t.timestamp_millis())
            .map_err(|_| {
                fail(format!(
                    "line {}: timestamp must be RFC3339 with timezone",
                    self.source_line
                ))
            })
    }
}
pub(super) struct JsonlEvents;
#[async_trait]
impl NodeExecutor for JsonlEvents {
    async fn execute(
        &self,
        n: &YamlNode,
        s: &ExecutionState,
        _: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let v = resolve_variables(&n.config.variables, s);
        let text = v
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("jsonl_events requires text"))?;
        if text.len() > 2 * 1024 * 1024 {
            return Err(fail("event log exceeds 2 MiB"));
        }
        let mut events = Vec::new();
        for (i, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if events.len() == LIMIT {
                return Err(fail("event log exceeds 10000 events"));
            }
            let mut value: Value = serde_json::from_str(line)
                .map_err(|_| fail(format!("line {}: invalid JSON", i + 1)))?;
            let obj = value
                .as_object_mut()
                .ok_or_else(|| fail(format!("line {}: expected object", i + 1)))?;
            // The source position is authoritative, never supplied by the log.
            obj.insert("source_line".into(), json!(i + 1));
            let event: Event = serde_json::from_value(value).map_err(|_| {
                fail(format!(
                    "line {}: expected entity, timestamp and action strings only",
                    i + 1
                ))
            })?;
            event.time()?;
            events.push(event);
        }
        Ok(HashMap::from([("events".into(), json!(events))]))
    }
}
pub(super) struct SessionizeEvents;
#[async_trait]
impl NodeExecutor for SessionizeEvents {
    async fn execute(
        &self,
        n: &YamlNode,
        s: &ExecutionState,
        _: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Options {
            idle_seconds: u32,
        }
        let o: Options = serde_json::from_value(json!(n.config.operation))
            .map_err(|_| fail("sessionize_events requires operation.idle_seconds"))?;
        if o.idle_seconds == 0 || o.idle_seconds > 86400 {
            return Err(fail("idle_seconds must be 1..86400"));
        }
        let mut vars = resolve_variables(&n.config.variables, s);
        let input = vars
            .remove("events")
            .ok_or_else(|| fail("sessionize_events requires events"))?;
        if input.as_array().map(|a| a.len() > LIMIT).unwrap_or(true) {
            return Err(fail("events must be an array of at most 10000 events"));
        }
        let events: Vec<Event> =
            serde_json::from_value(input).map_err(|_| fail("invalid normalized events"))?;
        let mut grouped: BTreeMap<String, Vec<(i64, Event)>> = BTreeMap::new();
        let mut lines = std::collections::HashSet::new();
        for e in events {
            if !lines.insert(e.source_line) {
                return Err(fail("duplicate source_line in normalized events"));
            }
            grouped
                .entry(e.entity.clone())
                .or_default()
                .push((e.time()?, e));
        }
        let entities = grouped.len();
        let mut sessions = Vec::new();
        let mut total = 0;
        for (entity, mut events) in grouped {
            events.sort_by_key(|(t, e)| (*t, e.source_line));
            total += events.len();
            let mut begin = 0;
            for end in 1..=events.len() {
                if end < events.len()
                    && events[end].0 - events[end - 1].0 <= i64::from(o.idle_seconds) * 1000
                {
                    continue;
                }
                let slice = &events[begin..end];
                sessions.push(json!({"entity":entity,"start":slice[0].1.timestamp,"end":slice[slice.len()-1].1.timestamp,
                    "duration_ms":slice[slice.len()-1].0-slice[0].0,"event_count":slice.len(),
                    "events":slice.iter().map(|(_,e)|e).collect::<Vec<_>>()}));
                begin = end;
            }
        }
        Ok(HashMap::from([(
            "report".into(),
            json!({"entity_count":entities,"event_count":total,"session_count":sessions.len(),"sessions":sessions}),
        )]))
    }
}
