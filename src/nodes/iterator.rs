//! Iterator node executor - processes an array by running a subgraph for each element
//!
//! Supports sequential and parallel execution with configurable concurrency.
//! Each iteration gets an isolated ExecutionState with the current item,
//! index, and any shared variables.

use std::collections::HashMap;
use std::time::Instant;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{debug, info};

use crate::schema::{YamlNode, YamlEdge, NodeType};
use crate::state::ExecutionState;
use crate::engine::ExecutionConfig;
use crate::error::EngineError;
use super::supervisor::{SupervisorTokenUsage, LLMCallUsage};
use super::{NodeExecutor, resolve_variables, resolve_single_binding};

pub struct IteratorExecutor;

/// Result of a single iteration
enum IterationResult {
    Success {
        #[allow(dead_code)]
        index: usize,
        outputs: Value,
        usage: SupervisorTokenUsage,
    },
    Error {
        index: usize,
        error: String,
    },
}

#[async_trait]
impl NodeExecutor for IteratorExecutor {
    async fn execute(
        &self,
        node: &YamlNode,
        state: &ExecutionState,
        config: &ExecutionConfig,
    ) -> Result<HashMap<String, Value>, EngineError> {
        let node_id = &node.id;

        // ── 1. Read & Validate Configuration ──────────────────────────
        let items_binding = node.config.items.as_ref()
            .ok_or_else(|| EngineError::MissingField(
                format!("Node '{}': 'items' is required for iterator node", node_id)
            ))?;

        let subgraph_ref = node.config.subgraph.as_ref()
            .ok_or_else(|| EngineError::MissingField(
                format!("Node '{}': 'subgraph' is required for iterator node", node_id)
            ))?;

        let item_var = node.config.item_var.as_deref().unwrap_or("item");
        let index_var = node.config.index_var.as_deref();
        let max_iterations = node.config.max_iterations.unwrap_or(1000);
        let is_parallel = node.config.parallel.unwrap_or(false);
        let max_concurrency = node.config.max_concurrency.unwrap_or(5) as usize;
        let on_error = node.config.on_error.as_deref().unwrap_or("fail_fast");

        // ── 2. Resolve Items → Must Be Array ──────────────────────────
        let items_value = resolve_single_binding(items_binding, state)
            .unwrap_or(Value::Null);

        let items_array = match items_value {
            Value::Array(arr) => arr,
            Value::Null => {
                debug!(node = %node_id, "Iterator items resolved to null, returning empty");
                return Ok(empty_outputs());
            }
            other => {
                let type_name = match &other {
                    Value::String(_) => "string",
                    Value::Number(_) => "number",
                    Value::Bool(_) => "boolean",
                    Value::Object(_) => "object",
                    _ => "unknown",
                };
                return Err(EngineError::IteratorInvalidInput(
                    format!("Node '{}': items resolved to {}, expected array", node_id, type_name)
                ));
            }
        };

        // ── 3. Empty Array → Early Return ─────────────────────────────
        if items_array.is_empty() {
            debug!(node = %node_id, "Iterator items is empty array, returning empty");
            return Ok(empty_outputs());
        }

        // ── 4. Safety Check ───────────────────────────────────────────
        if items_array.len() as u32 > max_iterations {
            return Err(EngineError::IteratorMaxIterations(max_iterations));
        }

        // ── 5. Validate Subgraph Exists ───────────────────────────────
        if !config.subgraphs.contains_key(subgraph_ref) {
            return Err(EngineError::IteratorSubgraphNotFound(subgraph_ref.clone()));
        }

        // ── 6. Resolve Shared Variables ───────────────────────────────
        let shared_vars = resolve_variables(&node.config.variables, state);

        info!(
            node = %node_id,
            items_count = items_array.len(),
            parallel = is_parallel,
            max_concurrency = max_concurrency,
            subgraph = %subgraph_ref,
            "Starting iterator execution"
        );

        let start_time = Instant::now();

        // ── 7. Execute Iterations ─────────────────────────────────────
        let results = if is_parallel && max_concurrency > 1 {
            execute_parallel(
                &items_array, subgraph_ref, config,
                item_var, index_var, &shared_vars,
                max_concurrency, on_error,
            ).await?
        } else {
            execute_sequential(
                &items_array, subgraph_ref, config,
                item_var, index_var, &shared_vars,
                on_error,
            ).await?
        };

        let elapsed_ms = start_time.elapsed().as_millis() as u64;

        // ── 8. Build Output ───────────────────────────────────────────
        let outputs = build_outputs(node_id, &results, items_array.len(), elapsed_ms);

        info!(
            node = %node_id,
            total_ms = elapsed_ms,
            items = items_array.len(),
            "Iterator execution completed"
        );

        Ok(outputs)
    }
}

/// Execute iterations sequentially
async fn execute_sequential(
    items: &[Value],
    subgraph_ref: &str,
    config: &ExecutionConfig,
    item_var: &str,
    index_var: Option<&str>,
    shared_vars: &HashMap<String, Value>,
    on_error: &str,
) -> Result<Vec<IterationResult>, EngineError> {
    let mut results = Vec::with_capacity(items.len());

    for (index, item) in items.iter().enumerate() {
        let result = execute_single_iteration(
            index, item, subgraph_ref, config,
            item_var, index_var, shared_vars,
        ).await;

        match result {
            Ok(r) => results.push(r),
            Err(e) => {
                if on_error == "fail_fast" {
                    return Err(EngineError::IteratorItemFailed {
                        index,
                        message: e.to_string(),
                    });
                }
                results.push(IterationResult::Error {
                    index,
                    error: e.to_string(),
                });
            }
        }
    }

    Ok(results)
}

/// Execute iterations in parallel with bounded concurrency
async fn execute_parallel(
    items: &[Value],
    subgraph_ref: &str,
    config: &ExecutionConfig,
    item_var: &str,
    index_var: Option<&str>,
    shared_vars: &HashMap<String, Value>,
    max_concurrency: usize,
    on_error: &str,
) -> Result<Vec<IterationResult>, EngineError> {
    use tokio::sync::Semaphore;
    use std::sync::Arc;

    let semaphore = Arc::new(Semaphore::new(max_concurrency));

    // Build all futures
    let futures: Vec<_> = items.iter().enumerate().map(|(index, item)| {
        let sem = semaphore.clone();
        let item = item.clone();
        let subgraph_ref = subgraph_ref.to_string();
        let item_var = item_var.to_string();
        let index_var = index_var.map(|s| s.to_string());
        let shared_vars = shared_vars.clone();
        // Config contains Arc fields so clone is cheap
        let config = config.clone();

        async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let result = execute_single_iteration(
                index, &item, &subgraph_ref, &config,
                &item_var, index_var.as_deref(), &shared_vars,
            ).await;
            (index, result)
        }
    }).collect();

    // Execute all with bounded concurrency
    let raw_results = futures::future::join_all(futures).await;

    // Sort by index to preserve input order
    let mut sorted: Vec<_> = raw_results.into_iter().collect();
    sorted.sort_by_key(|(idx, _)| *idx);

    // Process results
    let mut results = Vec::with_capacity(items.len());
    for (index, result) in sorted {
        match result {
            Ok(r) => results.push(r),
            Err(e) => {
                if on_error == "fail_fast" {
                    return Err(EngineError::IteratorItemFailed {
                        index,
                        message: e.to_string(),
                    });
                }
                results.push(IterationResult::Error {
                    index,
                    error: e.to_string(),
                });
            }
        }
    }

    Ok(results)
}

/// Execute a single iteration of the subgraph
async fn execute_single_iteration(
    index: usize,
    item: &Value,
    subgraph_ref: &str,
    config: &ExecutionConfig,
    item_var: &str,
    index_var: Option<&str>,
    shared_vars: &HashMap<String, Value>,
) -> Result<IterationResult, EngineError> {
    let subgraph = config.subgraphs.get(subgraph_ref)
        .ok_or_else(|| EngineError::IteratorSubgraphNotFound(subgraph_ref.to_string()))?;

    // Build input for this iteration
    let mut input: HashMap<String, Value> = HashMap::new();
    input.insert(item_var.to_string(), item.clone());
    if let Some(idx_var) = index_var {
        input.insert(idx_var.to_string(), Value::Number(index.into()));
    }
    for (name, value) in shared_vars {
        input.insert(name.clone(), value.clone());
    }

    // Create isolated execution state
    let mut state = ExecutionState::new(input);

    // Execute the subgraph (same pattern as supervisor.rs:696-841)
    let result = execute_subgraph_nodes(subgraph, &mut state, config).await?;

    Ok(IterationResult::Success {
        index,
        outputs: result.outputs,
        usage: result.usage,
    })
}

/// Result from executing a subgraph's nodes
struct SubgraphRunResult {
    outputs: Value,
    usage: SupervisorTokenUsage,
}

/// Execute all nodes in a subgraph using dependency-based scheduling
/// (Follows the same pattern as supervisor.rs execute_subgraph)
async fn execute_subgraph_nodes(
    subgraph: &crate::schema::Subgraph,
    state: &mut ExecutionState,
    config: &ExecutionConfig,
) -> Result<SubgraphRunResult, EngineError> {
    // Build node map and incoming edges
    let node_map: HashMap<&str, &YamlNode> = subgraph.nodes.iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    let incoming_edges: HashMap<&str, Vec<&YamlEdge>> = {
        let mut map: HashMap<&str, Vec<&YamlEdge>> = HashMap::new();
        for edge in &subgraph.edges {
            map.entry(edge.to.as_str()).or_default().push(edge);
        }
        map
    };

    // Check if node inputs are satisfied
    let inputs_satisfied = |node_id: &str, state: &ExecutionState| -> bool {
        let node = match node_map.get(node_id) {
            Some(n) => *n,
            None => return false,
        };

        if node.node_type == NodeType::Start {
            return true;
        }

        // Check variable bindings
        if !node.config.variables.is_empty() {
            for (_var_name, binding) in &node.config.variables {
                if binding.is_empty() || binding.has_default() {
                    continue;
                }
                let has_value = binding.sources().iter().any(|path| {
                    !path.is_empty() && state.has_output(path)
                });
                if !has_value {
                    return false;
                }
            }
            return true;
        }

        // Fall back to edge-based check
        let edges = incoming_edges.get(node_id).map(|v| v.as_slice()).unwrap_or(&[]);
        if edges.is_empty() {
            return true;
        }
        edges.iter().any(|e| state.has_node_output(&e.from))
    };

    // Initialize tracking
    let mut pending: std::collections::HashSet<String> = subgraph.nodes.iter()
        .map(|n| n.id.clone())
        .collect();
    let mut executed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut final_outputs: HashMap<String, Value> = HashMap::new();
    let mut total_usage = SupervisorTokenUsage::default();

    // Execute nodes in dependency order
    while !pending.is_empty() {
        let ready: Vec<String> = pending.iter()
            .filter(|id| !executed.contains(*id) && inputs_satisfied(id, state))
            .cloned()
            .collect();

        if ready.is_empty() {
            break;
        }

        for node_id in ready {
            let node = match node_map.get(node_id.as_str()) {
                Some(n) => *n,
                None => continue,
            };

            let executor = crate::nodes::create_executor(&node.node_type);
            let outputs = executor.execute(node, state, config).await?;

            // Extract token usage
            if let Some(usage_val) = outputs.get("_usage") {
                let provider = usage_val.get("provider")
                    .and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                let model = usage_val.get("model")
                    .and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                let input_tokens = usage_val.get("input_tokens")
                    .and_then(|v| v.as_u64()).unwrap_or(0);
                let output_tokens = usage_val.get("output_tokens")
                    .and_then(|v| v.as_u64()).unwrap_or(0);
                total_usage.add_from_llm_call(&LLMCallUsage::new(provider, model, input_tokens, output_tokens));
            }

            // Write to state
            state.write(&node.id, outputs.clone());

            // Track final outputs (response node preferred)
            if node.node_type == NodeType::Response {
                final_outputs = outputs;
            } else if final_outputs.is_empty() {
                final_outputs = outputs;
            }

            executed.insert(node_id.clone());
            pending.remove(&node_id);
        }
    }

    Ok(SubgraphRunResult {
        outputs: json!(final_outputs),
        usage: total_usage,
    })
}

/// Build output HashMap from iteration results
fn build_outputs(
    node_id: &str,
    results: &[IterationResult],
    total_count: usize,
    elapsed_ms: u64,
) -> HashMap<String, Value> {
    let mut outputs = HashMap::new();

    // results: array preserving input order
    let results_array: Vec<Value> = results.iter().map(|r| match r {
        IterationResult::Success { outputs, .. } => outputs.clone(),
        IterationResult::Error { error, .. } => json!({ "_error": error }),
    }).collect();
    outputs.insert("results".to_string(), Value::Array(results_array));

    // count
    outputs.insert("count".to_string(), Value::Number(total_count.into()));

    // success/error counts
    let error_count = results.iter().filter(|r| matches!(r, IterationResult::Error { .. })).count();
    let success_count = total_count - error_count;
    outputs.insert("success_count".to_string(), Value::Number(success_count.into()));
    outputs.insert("error_count".to_string(), Value::Number(error_count.into()));

    // errors array (for collect mode)
    let errors: Vec<Value> = results.iter().filter_map(|r| match r {
        IterationResult::Error { index, error } => Some(json!({
            "index": index,
            "error": error,
        })),
        _ => None,
    }).collect();
    if !errors.is_empty() {
        outputs.insert("errors".to_string(), Value::Array(errors));
    }

    // _usage: aggregated token usage
    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    for r in results {
        if let IterationResult::Success { usage, .. } = r {
            total_input += usage.input_tokens;
            total_output += usage.output_tokens;
        }
    }
    outputs.insert("_usage".to_string(), json!({
        "input_tokens": total_input,
        "output_tokens": total_output,
    }));

    debug!(
        node = %node_id,
        total_ms = elapsed_ms,
        success = success_count,
        errors = error_count,
        input_tokens = total_input,
        output_tokens = total_output,
        "Iterator outputs built"
    );

    outputs
}

/// Empty output for null/empty items
fn empty_outputs() -> HashMap<String, Value> {
    let mut outputs = HashMap::new();
    outputs.insert("results".to_string(), Value::Array(vec![]));
    outputs.insert("count".to_string(), Value::Number(0.into()));
    outputs.insert("success_count".to_string(), Value::Number(0.into()));
    outputs.insert("error_count".to_string(), Value::Number(0.into()));
    outputs.insert("_usage".to_string(), json!({
        "input_tokens": 0,
        "output_tokens": 0,
    }));
    outputs
}
