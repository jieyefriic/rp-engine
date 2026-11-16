//! Bounded, offline table operations. Logical record numbers include the header.
use super::{resolve_variables, NodeExecutor};
use crate::{EngineError, ExecutionConfig, ExecutionState, YamlNode};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_ROWS: usize = 10_000;
const MAX_COLS: usize = 128;
const MAX_CELLS: usize = 65_536;
const MAX_COLUMN_BYTES: usize = 128;

type Outputs = HashMap<String, Value>;

fn error(msg: impl Into<String>) -> EngineError {
    EngineError::NodeExecutionFailed(msg.into())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    fn validate(&self) -> Result<(), String> {
        let width = self.columns.len();
        if width == 0
            || width > MAX_COLS
            || self.rows.len() > MAX_ROWS
            || self.rows.len() * width > MAX_CELLS
        {
            return Err("invalid table dimensions or table limit exceeded".into());
        }
        let mut seen = HashSet::new();
        let mut bytes = 0;
        for col in &self.columns {
            if col.trim().is_empty() || col.len() > MAX_COLUMN_BYTES || !seen.insert(col) {
                return Err("column names must be unique, nonblank and at most 128 bytes".into());
            }
            bytes += col.len();
        }
        for (i, row) in self.rows.iter().enumerate() {
            if row.len() != width {
                return Err(format!(
                    "record {}: expected {} cells, found {}",
                    i + 2,
                    width,
                    row.len()
                ));
            }
            bytes += row.iter().map(String::len).sum::<usize>();
            if bytes > MAX_BYTES {
                return Err("table exceeds 2 MiB of text".into());
            }
        }
        Ok(())
    }
}

// Four lexical states: this rejects e.g. an unterminated quote instead of
// producing a plausible but incorrect diff. Only record separators are normalized.
#[derive(PartialEq, Eq)]
enum Field {
    Start,
    Plain,
    Quoted,
    Closed,
}

fn parse(text: &str) -> Result<Table, String> {
    if text.len() > MAX_BYTES {
        return Err("CSV exceeds 2 MiB".into());
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut chars = text.chars().peekable();
    let mut table = Table {
        columns: Vec::new(),
        rows: Vec::new(),
    };
    let mut row = Vec::new();
    let mut field = String::new();
    let mut mode = Field::Start;
    let mut open_record = false;
    let mut index = 1;
    while let Some(ch) = chars.next() {
        open_record = true;
        match (&mode, ch) {
            (Field::Quoted, '"') => mode = Field::Closed,
            (Field::Quoted, _) => field.push(ch),
            (Field::Closed, '"') => {
                field.push('"');
                mode = Field::Quoted;
            }
            (Field::Start, '"') => mode = Field::Quoted,
            (_, ',' | '\n' | '\r') => {
                row.push(std::mem::take(&mut field));
                if row.len() > MAX_COLS {
                    return Err(format!("record {index}: too many columns"));
                }
                mode = Field::Start;
                if ch == ',' {
                    continue;
                }
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                finish_record(&mut table, std::mem::take(&mut row), index)?;
                index += 1;
                open_record = false;
            }
            (Field::Closed, _) | (Field::Plain, '"') => {
                return Err(format!("record {index}: invalid quoting"))
            }
            _ => {
                mode = Field::Plain;
                field.push(ch);
            }
        }
    }
    if mode == Field::Quoted {
        return Err(format!("record {index}: unterminated quoted field"));
    }
    if open_record {
        row.push(field);
        finish_record(&mut table, row, index)?;
    }
    table.validate()?;
    Ok(table)
}

fn finish_record(table: &mut Table, row: Vec<String>, record: usize) -> Result<(), String> {
    if record == 1 {
        table.columns = row;
        return table.validate();
    }
    if row.len() != table.columns.len() {
        return Err(format!(
            "record {record}: expected {} cells, found {}",
            table.columns.len(),
            row.len()
        ));
    }
    if record - 1 > MAX_ROWS || (record - 1) * row.len() > MAX_CELLS {
        return Err("CSV row/cell limit exceeded".into());
    }
    table.rows.push(row);
    Ok(())
}

pub(super) struct CsvTable;
#[async_trait]
impl NodeExecutor for CsvTable {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        _: &ExecutionConfig,
    ) -> Result<Outputs, EngineError> {
        let vars = resolve_variables(&node.config.variables, state);
        let text = vars
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| error(format!("{}: expected string variable text", node.id)))?;
        let table = parse(text).map_err(|e| error(format!("{}: {e}", node.id)))?;
        Ok(HashMap::from([
            ("row_count".into(), json!(table.rows.len())),
            ("table".into(), json!(table)),
        ]))
    }
}

struct Side {
    table: Table,
    keys: Vec<usize>,
    compare: Vec<usize>,
}

impl Side {
    fn new(value: Value, keys: &[String], compare: &[String]) -> Result<Self, String> {
        let table: Table = serde_json::from_value(value)
            .map_err(|_| "expected a table with string columns and rows".to_string())?;
        table.validate()?;
        let positions = |cols: &[String]| {
            cols.iter()
                .map(|name| {
                    table
                        .columns
                        .iter()
                        .position(|c| c == name)
                        .ok_or_else(|| format!("missing configured column {name:?}"))
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let key_pos = positions(keys)?;
        let compare_pos = positions(compare)?;
        Ok(Self {
            table,
            keys: key_pos,
            compare: compare_pos,
        })
    }
    fn index(&self) -> Result<BTreeMap<Vec<&str>, Vec<usize>>, String> {
        let mut index = BTreeMap::new();
        for (i, row) in self.table.rows.iter().enumerate() {
            let key: Vec<&str> = self.keys.iter().map(|&col| row[col].as_str()).collect();
            if key.iter().any(|s| s.trim().is_empty()) {
                return Err(format!("record {}: blank key", i + 2));
            }
            index.entry(key).or_insert_with(Vec::new).push(i);
        }
        Ok(index)
    }
    fn values(&self, i: usize) -> BTreeMap<&str, &str> {
        self.compare
            .iter()
            .map(|&p| {
                (
                    self.table.columns[p].as_str(),
                    self.table.rows[i][p].as_str(),
                )
            })
            .collect()
    }
}

pub(super) struct ReconcileTables;
#[async_trait]
impl NodeExecutor for ReconcileTables {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        _: &ExecutionConfig,
    ) -> Result<Outputs, EngineError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Columns {
            keys: Vec<String>,
            compare: Vec<String>,
        }
        let opts: Columns = serde_json::from_value(Value::Object(
            node.config.operation.clone().into_iter().collect(),
        ))
        .map_err(|e| {
            error(format!(
                "{}: invalid reconciliation operation: {e}",
                node.id
            ))
        })?;
        let mut unique = HashSet::new();
        if opts.keys.is_empty()
            || opts.compare.is_empty()
            || opts
                .keys
                .iter()
                .chain(&opts.compare)
                .any(|c| !unique.insert(c))
        {
            return Err(error(format!(
                "{}: keys and compare must be nonempty, distinct column lists",
                node.id
            )));
        }
        let mut vars = resolve_variables(&node.config.variables, state);
        let mut side = |name: &str| {
            Side::new(
                vars.remove(name).unwrap_or(Value::Null),
                &opts.keys,
                &opts.compare,
            )
            .map_err(|e| error(format!("{}: {name}: {e}", node.id)))
        };
        let before = side("before")?;
        let after = side("after")?;
        let left = before
            .index()
            .map_err(|e| error(format!("{}: before: {e}", node.id)))?;
        let right = after
            .index()
            .map_err(|e| error(format!("{}: after: {e}", node.id)))?;
        let keys: std::collections::BTreeSet<_> = left.keys().chain(right.keys()).collect();
        let (mut added, mut removed, mut changed, mut conflicts) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut unchanged = 0usize;
        for k in keys {
            let key: BTreeMap<_, _> = opts.keys.iter().zip(k).collect();
            let l = left.get(k).map(Vec::as_slice).unwrap_or_default();
            let r = right.get(k).map(Vec::as_slice).unwrap_or_default();
            match (l, r) {
                ([], [j]) => added.push(json!({"key":key,"record":j+2,"values":after.values(*j)})),
                ([i], []) => {
                    removed.push(json!({"key":key,"record":i+2,"values":before.values(*i)}))
                }
                ([i], [j]) => {
                    let mut fields = BTreeMap::new();
                    for ((col, &x), &y) in
                        opts.compare.iter().zip(&before.compare).zip(&after.compare)
                    {
                        let (a, b) = (&before.table.rows[*i][x], &after.table.rows[*j][y]);
                        if a != b {
                            fields.insert(col, json!({"before":a,"after":b}));
                        }
                    }
                    if fields.is_empty() {
                        unchanged += 1;
                    } else {
                        changed.push(json!({"key":key,"before_record":i+2,"after_record":j+2,"fields":fields}));
                    }
                }
                _ => conflicts.push(
                    json!({"key":key,"before_records":l.iter().map(|i|i+2).collect::<Vec<_>>(),
                    "after_records":r.iter().map(|j|j+2).collect::<Vec<_>>()}),
                ),
            }
        }
        let summary = json!({"added":added.len(),"removed":removed.len(),"changed":changed.len(),"conflicts":conflicts.len(),"unchanged":unchanged});
        Ok(HashMap::from([(
            "report".into(),
            json!({"summary": summary,"added":added,"removed":removed,"changed":changed,"conflicts":conflicts}),
        )]))
    }
}
