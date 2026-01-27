//! Execution state with append-only semantics
//!
//! Each node execution generates a unique short_id and appends to history.
//! Paths support multiple reference formats:
//! - `node.field` - latest execution
//! - `node[a].field` - specific short_id
//! - `node[0].field` - first execution
//! - `node[-1].field` - last execution
//! - `node[*].field` - all executions (array)

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ExprError;

/// A single execution record for a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Short ID (e.g., "a", "b", "c", ...)
    pub short_id: String,

    /// Execution index (0, 1, 2, ...)
    pub execution_index: u32,

    /// Outputs from this execution
    pub outputs: HashMap<String, Value>,

    /// Timestamp in milliseconds
    pub timestamp: u64,
}

/// Supervisor context for dynamic task dispatch
#[derive(Debug, Clone, Default)]
pub struct SupervisorContext {
    /// Current todo item being processed
    pub current_todo: Option<Value>,

    /// Remaining pending items
    pub pending_todos: Vec<Value>,

    /// Completed results collected
    pub completed_results: Vec<Value>,
}

/// Execution state with append-only semantics
#[derive(Debug, Clone)]
pub struct ExecutionState {
    /// Node execution history: node_id -> Vec<ExecutionRecord>
    node_executions: HashMap<String, Vec<ExecutionRecord>>,

    /// Latest execution short_id for each node
    latest_execution: HashMap<String, String>,

    /// Workflow input (special namespace "input")
    input: HashMap<String, Value>,

    /// Edge execution counts for loop protection
    edge_counts: HashMap<(String, String), u32>,

    /// Supervisor context for dynamic dispatch
    supervisor_context: Option<SupervisorContext>,

    /// Counter for short ID generation
    short_id_counter: u32,

    /// Current variables (set by supervisor for edge conditions)
    current_vars: HashMap<String, Value>,
}

impl ExecutionState {
    /// Create a new execution state with initial input
    pub fn new(input: HashMap<String, Value>) -> Self {
        Self {
            node_executions: HashMap::new(),
            latest_execution: HashMap::new(),
            input,
            edge_counts: HashMap::new(),
            supervisor_context: None,
            short_id_counter: 0,
            current_vars: HashMap::new(),
        }
    }

    /// Generate a unique short ID using base36
    fn generate_short_id(&mut self) -> String {
        let id = to_base36(self.short_id_counter);
        self.short_id_counter += 1;
        id
    }

    /// Write node execution results (append-only, returns short_id)
    pub fn write(&mut self, node_id: &str, outputs: HashMap<String, Value>) -> String {
        let short_id = self.generate_short_id();
        let execution_index = self.node_executions
            .get(node_id)
            .map(|v| v.len() as u32)
            .unwrap_or(0);

        let record = ExecutionRecord {
            short_id: short_id.clone(),
            execution_index,
            outputs,
            timestamp: current_timestamp(),
        };

        self.node_executions
            .entry(node_id.to_string())
            .or_default()
            .push(record);

        self.latest_execution.insert(node_id.to_string(), short_id.clone());
        short_id
    }

    /// Read a value from state using path expression
    pub fn read(&self, path: &str) -> Option<Value> {
        self.resolve_path(path).ok()
    }

    /// Check if a path exists
    pub fn exists(&self, path: &str) -> bool {
        self.resolve_path(path).is_ok()
    }

    /// Get all executions for a node
    pub fn get_all_executions(&self, node_id: &str) -> &[ExecutionRecord] {
        self.node_executions
            .get(node_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Get the latest short_id for a node
    pub fn get_latest_short_id(&self, node_id: &str) -> Option<&str> {
        self.latest_execution.get(node_id).map(|s| s.as_str())
    }

    /// Get the latest execution record for a node
    pub fn get_latest_execution(&self, node_id: &str) -> Option<&ExecutionRecord> {
        self.node_executions.get(node_id).and_then(|v| v.last())
    }

    /// Get edge count for loop protection
    pub fn get_edge_count(&self, from: &str, to: &str) -> u32 {
        self.edge_counts
            .get(&(from.to_string(), to.to_string()))
            .copied()
            .unwrap_or(0)
    }

    /// Increment edge count
    pub fn increment_edge_count(&mut self, from: &str, to: &str) {
        let key = (from.to_string(), to.to_string());
        *self.edge_counts.entry(key).or_insert(0) += 1;
    }

    /// Set a variable (used by supervisor for edge conditions)
    pub fn set_var(&mut self, name: &str, value: Value) {
        self.current_vars.insert(name.to_string(), value);
    }

    /// Get a variable
    pub fn get_var(&self, name: &str) -> Option<&Value> {
        self.current_vars.get(name)
    }

    /// Check if supervisor context exists
    pub fn has_supervisor_context(&self) -> bool {
        self.supervisor_context.is_some()
    }

    /// Initialize supervisor context with todo list
    pub fn init_supervisor_context(&mut self, todos: Vec<Value>) {
        self.supervisor_context = Some(SupervisorContext {
            current_todo: None,
            pending_todos: todos,
            completed_results: Vec::new(),
        });
    }

    /// Get mutable supervisor context
    pub fn supervisor_context_mut(&mut self) -> Option<&mut SupervisorContext> {
        self.supervisor_context.as_mut()
    }

    /// Get supervisor context
    pub fn supervisor_context(&self) -> Option<&SupervisorContext> {
        self.supervisor_context.as_ref()
    }

    /// Pop the next todo from supervisor context
    pub fn pop_next_todo(&mut self) -> Option<Value> {
        self.supervisor_context.as_mut().and_then(|ctx| {
            if ctx.pending_todos.is_empty() {
                None
            } else {
                let todo = ctx.pending_todos.remove(0);
                ctx.current_todo = Some(todo.clone());
                Some(todo)
            }
        })
    }

    /// Append a result to supervisor context
    pub fn append_supervisor_result(&mut self, result: Value) {
        if let Some(ctx) = self.supervisor_context.as_mut() {
            ctx.completed_results.push(result);
        }
    }

    /// Get the full execution history
    pub fn get_executions(&self) -> &HashMap<String, Vec<ExecutionRecord>> {
        &self.node_executions
    }

    /// Get the input values
    pub fn get_input(&self) -> &HashMap<String, Value> {
        &self.input
    }

    /// Check if a node has been executed (has any output)
    pub fn has_node_output(&self, node_id: &str) -> bool {
        self.node_executions.contains_key(node_id)
            && !self.node_executions.get(node_id).map(|v| v.is_empty()).unwrap_or(true)
    }

    /// Check if a specific output path exists
    /// Path format: "node_id.field" or "input.field"
    pub fn has_output(&self, path: &str) -> bool {
        self.resolve_path(path).is_ok()
    }

    /// Get output value for a path, returns None if not found
    /// Path format: "node_id.field" or "input.field"
    pub fn get_output(&self, path: &str) -> Option<Value> {
        self.resolve_path(path).ok()
    }

    /// Resolve a path expression to a value
    pub fn resolve_path(&self, path: &str) -> Result<Value, ExprError> {
        let segments = parse_path(path)?;
        self.resolve_segments(&segments)
    }

    fn resolve_segments(&self, segments: &[PathSegment]) -> Result<Value, ExprError> {
        if segments.is_empty() {
            return Err(ExprError::InvalidPath("empty path".into()));
        }

        // First segment must be a field (node_id or "input" or variable)
        let root = match &segments[0] {
            PathSegment::Field(name) => name,
            _ => return Err(ExprError::InvalidPath("path must start with identifier".into())),
        };

        // Check for "input" namespace
        if root == "input" {
            return self.resolve_input_path(&segments[1..]);
        }

        // Check for current variables (set by supervisor)
        if let Some(value) = self.current_vars.get(root) {
            return self.resolve_value_path(value.clone(), &segments[1..]);
        }

        // Resolve node reference
        let executions = self.node_executions.get(root)
            .ok_or_else(|| ExprError::NodeNotFound(root.clone()))?;

        if executions.is_empty() {
            return Err(ExprError::NoExecutions);
        }

        // Check for selector after node_id
        if segments.len() < 2 {
            // Just node_id, return latest outputs
            let latest = executions.last().unwrap();
            return Ok(serde_json::to_value(&latest.outputs)?);
        }

        match &segments[1] {
            PathSegment::Index(idx) => {
                let record = self.get_by_index(executions, *idx)?;
                self.resolve_from_outputs(&record.outputs, &segments[2..])
            }
            PathSegment::ShortId(sid) => {
                let record = executions.iter()
                    .find(|r| r.short_id == *sid)
                    .ok_or_else(|| ExprError::ShortIdNotFound(sid.clone()))?;
                self.resolve_from_outputs(&record.outputs, &segments[2..])
            }
            PathSegment::All => {
                // Collect from all executions
                self.resolve_all_executions(executions, &segments[2..])
            }
            PathSegment::Field(field) => {
                // No selector, use latest execution
                let latest = executions.last().unwrap();
                self.resolve_from_outputs_with_field(&latest.outputs, field, &segments[2..])
            }
        }
    }

    fn resolve_input_path(&self, segments: &[PathSegment]) -> Result<Value, ExprError> {
        if segments.is_empty() {
            return Ok(serde_json::to_value(&self.input)?);
        }

        match &segments[0] {
            PathSegment::Field(field) => {
                let value = self.input.get(field)
                    .ok_or_else(|| ExprError::InvalidPath(format!("input.{} not found", field)))?;
                self.resolve_value_path(value.clone(), &segments[1..])
            }
            _ => Err(ExprError::InvalidPath("invalid input path".into())),
        }
    }

    fn resolve_from_outputs(&self, outputs: &HashMap<String, Value>, segments: &[PathSegment]) -> Result<Value, ExprError> {
        if segments.is_empty() {
            return Ok(serde_json::to_value(outputs)?);
        }

        match &segments[0] {
            PathSegment::Field(field) => {
                let value = outputs.get(field)
                    .ok_or_else(|| ExprError::InvalidPath(format!("field {} not found", field)))?;
                self.resolve_value_path(value.clone(), &segments[1..])
            }
            _ => Err(ExprError::InvalidPath("expected field after selector".into())),
        }
    }

    fn resolve_from_outputs_with_field(&self, outputs: &HashMap<String, Value>, field: &str, remaining: &[PathSegment]) -> Result<Value, ExprError> {
        let value = outputs.get(field)
            .ok_or_else(|| ExprError::InvalidPath(format!("field {} not found", field)))?;
        self.resolve_value_path(value.clone(), remaining)
    }

    fn resolve_all_executions(&self, executions: &[ExecutionRecord], segments: &[PathSegment]) -> Result<Value, ExprError> {
        if segments.is_empty() {
            // Return all outputs as array
            let all: Vec<_> = executions.iter().map(|r| &r.outputs).collect();
            return Ok(serde_json::to_value(all)?);
        }

        // Get field from each execution
        match &segments[0] {
            PathSegment::Field(field) => {
                let values: Vec<Value> = executions.iter()
                    .filter_map(|r| r.outputs.get(field).cloned())
                    .collect();
                if segments.len() == 1 {
                    Ok(Value::Array(values))
                } else {
                    // Further path resolution on array not supported yet
                    Err(ExprError::InvalidPath("nested path after [*] not fully supported".into()))
                }
            }
            _ => Err(ExprError::InvalidPath("expected field after [*]".into())),
        }
    }

    fn resolve_value_path(&self, value: Value, segments: &[PathSegment]) -> Result<Value, ExprError> {
        if segments.is_empty() {
            return Ok(value);
        }

        match &segments[0] {
            PathSegment::Field(field) => {
                match value {
                    Value::Object(map) => {
                        let next = map.get(field)
                            .ok_or_else(|| ExprError::InvalidPath(format!("field {} not found", field)))?;
                        self.resolve_value_path(next.clone(), &segments[1..])
                    }
                    _ => Err(ExprError::InvalidPath("cannot access field on non-object".into())),
                }
            }
            PathSegment::Index(idx) => {
                match value {
                    Value::Array(arr) => {
                        let len = arr.len() as i32;
                        let actual_idx = if *idx < 0 { len + *idx } else { *idx };
                        if actual_idx < 0 || actual_idx >= len {
                            return Err(ExprError::IndexOutOfBounds(*idx));
                        }
                        self.resolve_value_path(arr[actual_idx as usize].clone(), &segments[1..])
                    }
                    _ => Err(ExprError::InvalidPath("cannot index non-array".into())),
                }
            }
            _ => Err(ExprError::InvalidPath("unexpected segment".into())),
        }
    }

    fn get_by_index<'a>(&self, executions: &'a [ExecutionRecord], idx: i32) -> Result<&'a ExecutionRecord, ExprError> {
        let len = executions.len() as i32;
        let actual_idx = if idx < 0 { len + idx } else { idx };
        if actual_idx < 0 || actual_idx >= len {
            return Err(ExprError::IndexOutOfBounds(idx));
        }
        Ok(&executions[actual_idx as usize])
    }
}

/// Path segment types
#[derive(Debug, Clone)]
pub enum PathSegment {
    Field(String),
    Index(i32),
    ShortId(String),
    All,
}

/// Parse a path string into segments
pub fn parse_path(path: &str) -> Result<Vec<PathSegment>, ExprError> {
    let mut segments = Vec::new();
    let mut chars = path.chars().peekable();
    let mut current = String::new();

    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !current.is_empty() {
                    segments.push(PathSegment::Field(current.clone()));
                    current.clear();
                }
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(PathSegment::Field(current.clone()));
                    current.clear();
                }
                // Read until ']'
                let mut selector = String::new();
                loop {
                    match chars.next() {
                        Some(']') => break,
                        Some(ch) => selector.push(ch),
                        None => return Err(ExprError::InvalidPath("unclosed bracket".into())),
                    }
                }
                segments.push(parse_selector(&selector)?);
            }
            _ => current.push(c),
        }
    }

    if !current.is_empty() {
        segments.push(PathSegment::Field(current));
    }

    Ok(segments)
}

/// Parse a selector string
fn parse_selector(s: &str) -> Result<PathSegment, ExprError> {
    if s == "*" {
        Ok(PathSegment::All)
    } else if s.starts_with('-') || s.chars().all(|c| c.is_ascii_digit()) {
        s.parse::<i32>()
            .map(PathSegment::Index)
            .map_err(|_| ExprError::ParseError(format!("invalid index: {}", s)))
    } else {
        Ok(PathSegment::ShortId(s.to_string()))
    }
}

/// Convert number to base36 string
fn to_base36(mut n: u32) -> String {
    if n == 0 {
        return "a".to_string();
    }

    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut result = Vec::new();

    while n > 0 {
        result.push(CHARS[(n % 36) as usize]);
        n /= 36;
    }

    result.reverse();
    String::from_utf8(result).unwrap()
}

/// Get current timestamp in milliseconds
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_id_generation() {
        assert_eq!(to_base36(0), "a");
        assert_eq!(to_base36(1), "b");
        assert_eq!(to_base36(25), "z");
        assert_eq!(to_base36(26), "0");
        assert_eq!(to_base36(35), "9");
        assert_eq!(to_base36(36), "ba");
    }

    #[test]
    fn test_write_and_read() {
        let mut state = ExecutionState::new(HashMap::new());

        let mut outputs = HashMap::new();
        outputs.insert("result".to_string(), Value::String("hello".to_string()));

        let short_id = state.write("node1", outputs);
        assert_eq!(short_id, "a");

        let value = state.read("node1.result").unwrap();
        assert_eq!(value, Value::String("hello".to_string()));
    }

    #[test]
    fn test_multiple_executions() {
        let mut state = ExecutionState::new(HashMap::new());

        // First execution
        let mut outputs1 = HashMap::new();
        outputs1.insert("count".to_string(), Value::Number(1.into()));
        let id1 = state.write("counter", outputs1);

        // Second execution
        let mut outputs2 = HashMap::new();
        outputs2.insert("count".to_string(), Value::Number(2.into()));
        let id2 = state.write("counter", outputs2);

        // Latest (default)
        assert_eq!(state.read("counter.count"), Some(Value::Number(2.into())));

        // By short_id
        assert_eq!(state.read(&format!("counter[{}].count", id1)), Some(Value::Number(1.into())));
        assert_eq!(state.read(&format!("counter[{}].count", id2)), Some(Value::Number(2.into())));

        // By index
        assert_eq!(state.read("counter[0].count"), Some(Value::Number(1.into())));
        assert_eq!(state.read("counter[-1].count"), Some(Value::Number(2.into())));

        // All executions
        let all = state.read("counter[*].count").unwrap();
        assert_eq!(all, Value::Array(vec![Value::Number(1.into()), Value::Number(2.into())]));
    }

    #[test]
    fn test_input_namespace() {
        let mut input = HashMap::new();
        input.insert("query".to_string(), Value::String("test query".to_string()));

        let state = ExecutionState::new(input);

        assert_eq!(state.read("input.query"), Some(Value::String("test query".to_string())));
    }

    #[test]
    fn test_path_parsing() {
        let segments = parse_path("node.field.subfield").unwrap();
        assert_eq!(segments.len(), 3);

        let segments = parse_path("node[0].field").unwrap();
        assert_eq!(segments.len(), 3);

        let segments = parse_path("node[abc].field").unwrap();
        assert_eq!(segments.len(), 3);

        let segments = parse_path("node[*].field").unwrap();
        assert_eq!(segments.len(), 3);
    }
}
