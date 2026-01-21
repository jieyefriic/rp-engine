//! Main execution engine with layer-based parallel execution

use std::collections::{HashMap, HashSet, BTreeMap};
use std::sync::Arc;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use deadpool_postgres::{Pool as PgPool, Config as PgConfig, Runtime};

use crate::error::EngineError;
use crate::schema::{PreparedWorkflow, Template, YamlNode, NodeType, ExitReason, DataSourceDef};
use crate::state::{ExecutionState, ExecutionRecord};
use crate::expression::evaluate_expression;
use crate::providers::{ProviderRegistry, EmbeddingRegistry};
use crate::nodes::{create_executor, NodeRegistry};
use super::graph::ExecutionGraph;

/// Execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    Success,
    Error,
    Timeout,
}

/// Timing information for execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionTiming {
    /// Total execution time in milliseconds
    pub total_ms: u64,
    /// Time per node: node_id -> milliseconds
    pub per_node: HashMap<String, u64>,
    /// Time per layer: layer -> milliseconds
    pub per_layer: HashMap<u32, u64>,
}

/// Token usage tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub per_node: HashMap<String, (u64, u64)>,
}

/// Configuration for database connection pools
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum pool size (default: 16)
    pub max_size: usize,
    /// Minimum idle connections (default: 2)
    pub min_idle: Option<usize>,
    /// Connection timeout in seconds (default: 30)
    pub connection_timeout_secs: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 16,
            min_idle: Some(2),
            connection_timeout_secs: 30,
        }
    }
}

/// Connection pools for various data sources
#[derive(Default)]
pub struct ConnectionPools {
    /// PostgreSQL pools keyed by data source ID
    pub postgresql: HashMap<String, PgPool>,
}

impl ConnectionPools {
    /// Create a new empty connection pools container
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a PostgreSQL pool from data source config
    pub async fn create_pg_pool(
        ds_config: &HashMap<String, Value>,
        pool_config: &PoolConfig,
    ) -> Result<PgPool, EngineError> {
        let mut pg_cfg = PgConfig::new();

        // Set connection parameters from data source config
        if let Some(url) = ds_config.get("url").and_then(|v| v.as_str()) {
            // Parse connection URL
            pg_cfg.url = Some(url.to_string());
        } else {
            // Build from individual parameters
            if let Some(host) = ds_config.get("host").and_then(|v| v.as_str()) {
                pg_cfg.host = Some(host.to_string());
            }
            if let Some(port) = ds_config.get("port").and_then(|v| v.as_u64()) {
                pg_cfg.port = Some(port as u16);
            }
            if let Some(user) = ds_config.get("username").and_then(|v| v.as_str()) {
                pg_cfg.user = Some(user.to_string());
            }
            if let Some(password) = ds_config.get("password").and_then(|v| v.as_str()) {
                pg_cfg.password = Some(password.to_string());
            }
            if let Some(dbname) = ds_config.get("database").and_then(|v| v.as_str()) {
                pg_cfg.dbname = Some(dbname.to_string());
            }
        }

        // Set pool size
        pg_cfg.pool = Some(deadpool_postgres::PoolConfig {
            max_size: pool_config.max_size,
            ..Default::default()
        });

        // Create pool
        let pool = pg_cfg.create_pool(Some(Runtime::Tokio1), tokio_postgres::NoTls)
            .map_err(|e| EngineError::DatabaseError(format!("Failed to create pool: {}", e)))?;

        Ok(pool)
    }

    /// Get a PostgreSQL pool by data source ID
    pub fn get_pg_pool(&self, data_source_id: &str) -> Option<&PgPool> {
        self.postgresql.get(data_source_id)
    }

    /// Register a PostgreSQL pool for a data source
    pub fn register_pg_pool(&mut self, data_source_id: String, pool: PgPool) {
        self.postgresql.insert(data_source_id, pool);
    }
}

/// Execution configuration
#[derive(Clone)]
pub struct ExecutionConfig {
    /// Provider registry for LLM calls
    pub providers: ProviderRegistry,

    /// Embedding provider registry for embedding calls
    pub embedding_providers: EmbeddingRegistry,

    /// Global timeout
    pub timeout: Duration,

    /// Maximum iterations for any edge (global limit)
    pub max_iterations: u32,

    /// Templates from the workflow (passed separately for ownership)
    pub templates: HashMap<String, Template>,

    /// Subgraphs (ToolBoxes) from the workflow
    pub subgraphs: HashMap<String, crate::schema::Subgraph>,

    /// Data Sources from the workflow (data_source_id -> DataSourceDef)
    pub data_sources: HashMap<String, DataSourceDef>,

    /// Custom node registry for user-defined node types
    pub node_registry: Arc<NodeRegistry>,

    /// Connection pools for databases
    pub pools: Arc<ConnectionPools>,

    /// Whether to capture the actual rendered prompts (for observability)
    pub capture_actual_prompt: bool,

    /// Whether to capture the raw LLM response before parsing (for observability)
    pub capture_raw_response: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            providers: ProviderRegistry::new(),
            embedding_providers: EmbeddingRegistry::new(),
            timeout: Duration::from_secs(300), // 5 minutes
            max_iterations: 100,
            templates: HashMap::new(),
            subgraphs: HashMap::new(),
            data_sources: HashMap::new(),
            node_registry: Arc::new(NodeRegistry::new()),
            pools: Arc::new(ConnectionPools::new()),
            capture_actual_prompt: false,
            capture_raw_response: false,
        }
    }
}

impl ExecutionConfig {
    /// Create config with providers
    pub fn with_providers(mut self, providers: ProviderRegistry) -> Self {
        self.providers = providers;
        self
    }

    /// Set embedding providers
    pub fn with_embedding_providers(mut self, embedding_providers: EmbeddingRegistry) -> Self {
        self.embedding_providers = embedding_providers;
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set max iterations
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = max;
        self
    }

    /// Set templates
    pub fn with_templates(mut self, templates: HashMap<String, Template>) -> Self {
        self.templates = templates;
        self
    }

    /// Set data sources
    pub fn with_data_sources(mut self, data_sources: HashMap<String, DataSourceDef>) -> Self {
        self.data_sources = data_sources;
        self
    }

    /// Set custom node registry
    pub fn with_node_registry(mut self, registry: Arc<NodeRegistry>) -> Self {
        self.node_registry = registry;
        self
    }

    /// Set connection pools
    pub fn with_pools(mut self, pools: Arc<ConnectionPools>) -> Self {
        self.pools = pools;
        self
    }

    /// Enable/disable capturing of actual rendered prompts
    pub fn with_capture_actual_prompt(mut self, enabled: bool) -> Self {
        self.capture_actual_prompt = enabled;
        self
    }

    /// Enable/disable capturing of raw LLM response
    pub fn with_capture_raw_response(mut self, enabled: bool) -> Self {
        self.capture_raw_response = enabled;
        self
    }
}

/// Result of workflow execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Execution status
    pub status: ExecutionStatus,

    /// Complete execution history
    pub executions: HashMap<String, Vec<ExecutionRecord>>,

    /// Final output (from response node)
    pub output: HashMap<String, Value>,

    /// Error message if failed
    pub error: Option<String>,

    /// Timing information
    pub timing: ExecutionTiming,

    /// Token usage
    pub token_usage: TokenUsage,

    /// Exit reason from Supervisor node (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_reason: Option<ExitReason>,
}

/// Result of a single node execution
struct NodeResult {
    node_id: String,
    outputs: HashMap<String, Value>,
    timing_ms: u64,
}

/// Execute a workflow
pub async fn execute(
    workflow_yaml: &str,
    input: HashMap<String, Value>,
    config: ExecutionConfig,
) -> Result<ExecutionResult, EngineError> {
    // Parse workflow
    let workflow = PreparedWorkflow::from_yaml(workflow_yaml)
        .map_err(|e| EngineError::InvalidWorkflow(e.to_string()))?;

    execute_workflow(&workflow, input, config).await
}

/// Execute a parsed workflow with layer-based parallel execution
pub async fn execute_workflow(
    workflow: &PreparedWorkflow,
    input: HashMap<String, Value>,
    mut config: ExecutionConfig,
) -> Result<ExecutionResult, EngineError> {
    let start_time = Instant::now();

    // Validate input against workflow schema
    workflow.input.validate(&input)?;

    // Merge workflow templates into config
    for (id, template) in &workflow.templates {
        config.templates.insert(id.clone(), template.clone());
    }

    // Merge workflow subgraphs into config
    for (id, subgraph) in &workflow.subgraphs {
        config.subgraphs.insert(id.clone(), subgraph.clone());
    }

    // Merge workflow data sources into config
    for (id, data_source) in &workflow.data_sources {
        config.data_sources.insert(id.clone(), data_source.clone());
    }

    // Wrap config in Arc for sharing across tasks
    let config = Arc::new(config);

    let state = Arc::new(RwLock::new(ExecutionState::new(input)));
    let graph = ExecutionGraph::from_workflow(workflow);

    // Build layer map: layer -> Vec<node_id>
    let layer_map = build_layer_map(workflow);
    let max_layer = layer_map.keys().max().copied().unwrap_or(0);

    let mut executed: HashSet<String> = HashSet::new();
    let mut timing = ExecutionTiming::default();
    let mut token_usage = TokenUsage::default();
    let mut final_output: HashMap<String, Value> = HashMap::new();
    let mut pending_nodes: HashSet<String> = HashSet::new();

    // Start with layer 0
    if let Some(nodes) = layer_map.get(&0) {
        for node_id in nodes {
            pending_nodes.insert(node_id.clone());
        }
    }

    // Execute layers in order
    let mut current_layer = 0u32;

    while !pending_nodes.is_empty() || current_layer <= max_layer {
        // Check timeout
        if start_time.elapsed() > config.timeout {
            let state_guard = state.read().await;
            return Ok(ExecutionResult {
                status: ExecutionStatus::Timeout,
                executions: state_guard.get_executions().clone(),
                output: final_output,
                error: Some("Execution timeout".into()),
                timing,
                token_usage,
                exit_reason: None,
            });
        }

        // Get nodes ready to execute in this layer
        let ready_nodes: Vec<String> = {
            let _state_guard = state.read().await;
            pending_nodes.iter()
                .filter(|node_id| {
                    let node = graph.get_node(node_id).ok();
                    node.map(|n| n.layer == current_layer).unwrap_or(false)
                        && graph.dependencies_met(node_id, &executed)
                        && !executed.contains(*node_id)
                })
                .cloned()
                .collect()
        };

        if ready_nodes.is_empty() {
            // No nodes ready in this layer, move to next
            current_layer += 1;

            // Check if there are any pending nodes in higher layers
            let has_pending_in_higher = pending_nodes.iter().any(|id| {
                graph.get_node(id).map(|n| n.layer >= current_layer).unwrap_or(false)
            });

            if !has_pending_in_higher && current_layer > max_layer {
                break;
            }
            continue;
        }

        let layer_start = Instant::now();

        // Execute all ready nodes in this layer in PARALLEL
        let results = execute_nodes_parallel(
            &ready_nodes,
            &graph,
            state.clone(),
            config.clone(),
        ).await?;

        // Record layer timing
        timing.per_layer.insert(current_layer, layer_start.elapsed().as_millis() as u64);

        // Process results
        let mut state_guard = state.write().await;

        for result in results {
            // Record node timing
            timing.per_node.insert(result.node_id.clone(), result.timing_ms);

            // Track token usage
            if let Some(usage) = result.outputs.get("_usage") {
                if let (Some(input_tokens), Some(output_tokens)) = (
                    usage.get("input_tokens").and_then(|v| v.as_u64()),
                    usage.get("output_tokens").and_then(|v| v.as_u64()),
                ) {
                    token_usage.total_input_tokens += input_tokens;
                    token_usage.total_output_tokens += output_tokens;
                    token_usage.per_node.insert(result.node_id.clone(), (input_tokens, output_tokens));
                }
            }

            // Check for exit_reason from Supervisor nodes (before moving outputs)
            let exit_reason = if let Ok(node) = graph.get_node(&result.node_id) {
                if node.node_type == NodeType::Supervisor {
                    result.outputs.get("exit_reason")
                        .and_then(|v| serde_json::from_value::<ExitReason>(v.clone()).ok())
                } else {
                    None
                }
            } else {
                None
            };

            // Write outputs to state
            let short_id = state_guard.write(&result.node_id, result.outputs.clone());
            tracing::debug!(node = %result.node_id, short_id = %short_id, layer = current_layer, "Node executed");

            // If this is a response node, capture final output
            if graph.is_response_node(&result.node_id) {
                final_output = result.outputs;
            }

            // Mark as executed
            executed.insert(result.node_id.clone());
            pending_nodes.remove(&result.node_id);

            // Evaluate outgoing edges and add next nodes to pending
            let next_nodes = evaluate_edges(&graph, &result.node_id, &mut state_guard, &config, exit_reason)?;
            for next in next_nodes {
                if !executed.contains(&next) {
                    pending_nodes.insert(next);
                }
            }
        }

        drop(state_guard);
    }

    timing.total_ms = start_time.elapsed().as_millis() as u64;

    let state_guard = state.read().await;
    Ok(ExecutionResult {
        status: ExecutionStatus::Success,
        executions: state_guard.get_executions().clone(),
        output: final_output,
        error: None,
        timing,
        token_usage,
        exit_reason: None,
    })
}

/// Build a map of layer -> node_ids
fn build_layer_map(workflow: &PreparedWorkflow) -> BTreeMap<u32, Vec<String>> {
    let mut map: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for node in &workflow.nodes {
        map.entry(node.layer).or_default().push(node.id.clone());
    }
    map
}

/// Execute multiple nodes in parallel
async fn execute_nodes_parallel(
    node_ids: &[String],
    graph: &ExecutionGraph<'_>,
    state: Arc<RwLock<ExecutionState>>,
    config: Arc<ExecutionConfig>,
) -> Result<Vec<NodeResult>, EngineError> {
    use futures::future::join_all;

    // Prepare node data for parallel execution
    let tasks: Vec<_> = node_ids.iter().map(|node_id| {
        let node_id = node_id.clone();
        let node = graph.get_node(&node_id).unwrap().clone();
        let state = state.clone();
        let config = config.clone();

        async move {
            execute_single_node(node, state, config).await
        }
    }).collect();

    // Execute all in parallel
    let results = join_all(tasks).await;

    // Collect results, fail on first error
    let mut node_results = Vec::new();
    for result in results {
        node_results.push(result?);
    }

    Ok(node_results)
}

/// Execute a single node
async fn execute_single_node(
    node: YamlNode,
    state: Arc<RwLock<ExecutionState>>,
    config: Arc<ExecutionConfig>,
) -> Result<NodeResult, EngineError> {
    let node_start = Instant::now();
    let node_id = node.id.clone();

    // Get read-only snapshot of state for execution
    let state_snapshot = {
        let guard = state.read().await;
        guard.clone()
    };

    // Execute based on node type
    let outputs = match &node.node_type {
        NodeType::Custom(type_name) => {
            // Look up custom node in registry
            match config.node_registry.get(type_name) {
                Some(custom_node) => {
                    // Execute custom node directly with NodeConfig
                    custom_node.execute(&node.config, &state_snapshot, &config).await?
                }
                None => {
                    // Fall back to built-in executor (which will return an error)
                    let executor = create_executor(&node.node_type);
                    executor.execute(&node, &state_snapshot, &config).await?
                }
            }
        }
        _ => {
            // Use built-in executor for standard node types
            let executor = create_executor(&node.node_type);
            executor.execute(&node, &state_snapshot, &config).await?
        }
    };

    let timing_ms = node_start.elapsed().as_millis() as u64;

    Ok(NodeResult {
        node_id,
        outputs,
        timing_ms,
    })
}

/// Evaluate outgoing edges and return next nodes to execute
fn evaluate_edges(
    graph: &ExecutionGraph,
    from_node: &str,
    state: &mut ExecutionState,
    config: &ExecutionConfig,
    exit_reason: Option<ExitReason>,
) -> Result<Vec<String>, EngineError> {
    let edges = graph.get_outgoing_edges(from_node);
    let mut next = Vec::new();

    // If we have an exit_reason (from Supervisor), prioritize exit edge matching
    if let Some(reason) = exit_reason {
        // Collect exit edges
        let exit_edges: Vec<_> = edges.iter()
            .filter(|e| e.exit_reason.is_some())
            .collect();

        if !exit_edges.is_empty() {
            // First, try exact match
            let exact_matches: Vec<_> = exit_edges.iter()
                .filter(|e| e.exit_reason == Some(reason))
                .collect();

            if !exact_matches.is_empty() {
                for edge in exact_matches {
                    let edge_max = edge.max_iterations.unwrap_or(config.max_iterations);
                    let count = state.get_edge_count(&edge.from, &edge.to);
                    if count < edge_max {
                        tracing::debug!(
                            from = %edge.from, to = %edge.to, exit_reason = ?reason,
                            "Taking exit edge with exact match"
                        );
                        next.push(edge.to.clone());
                        state.increment_edge_count(&edge.from, &edge.to);
                    }
                }
                if !next.is_empty() {
                    return Ok(next);
                }
            }

            // Fall back to Default exit reason
            let default_matches: Vec<_> = exit_edges.iter()
                .filter(|e| e.exit_reason == Some(ExitReason::Default))
                .collect();

            if !default_matches.is_empty() {
                for edge in default_matches {
                    let edge_max = edge.max_iterations.unwrap_or(config.max_iterations);
                    let count = state.get_edge_count(&edge.from, &edge.to);
                    if count < edge_max {
                        tracing::debug!(
                            from = %edge.from, to = %edge.to, actual_reason = ?reason,
                            "Taking default exit edge"
                        );
                        next.push(edge.to.clone());
                        state.increment_edge_count(&edge.from, &edge.to);
                    }
                }
                if !next.is_empty() {
                    return Ok(next);
                }
            }
        }
        // If no exit edges found, fall through to regular edge evaluation
    }

    // Regular edge evaluation (for non-Supervisor nodes or when no exit edges matched)
    //
    // Edge types and behavior:
    // 1. is_default: true  → ALWAYS execute (unconditionally)
    // 2. condition: expr   → Execute if expr evaluates to true
    // 3. No condition, not default → Execute unconditionally

    for edge in &edges {
        // Skip exit edges in regular evaluation
        if edge.exit_reason.is_some() {
            continue;
        }

        // Check max_iterations
        let edge_max = edge.max_iterations.unwrap_or(config.max_iterations);
        let count = state.get_edge_count(&edge.from, &edge.to);

        if count >= edge_max {
            tracing::debug!(
                from = %edge.from, to = %edge.to, count = count, max = edge_max,
                "Edge max iterations reached, skipping"
            );
            continue;
        }

        // Determine if edge should be taken
        let should_take = if edge.is_default {
            // Default edges ALWAYS execute
            true
        } else if let Some(expr) = &edge.condition {
            // Conditional edges: evaluate the expression
            match evaluate_expression(expr, state) {
                Ok(result) => result,
                Err(e) => {
                    tracing::warn!(expr = %expr, error = %e, "Edge condition evaluation failed");
                    false
                }
            }
        } else {
            // Unconditional non-default edges: always execute
            true
        };

        if should_take {
            next.push(edge.to.clone());
            state.increment_edge_count(&edge.from, &edge.to);
        }
    }

    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_simple_workflow() {
        let yaml = r#"
version: "0.1"
name: "Simple"
nodes:
  - id: start
    type: start
    layer: 0
    config:
      output:
        message: "input.query"
  - id: response
    type: response
    layer: 1
edges:
  - from: start
    to: response
"#;
        let mut input = HashMap::new();
        input.insert("query".to_string(), Value::String("hello".to_string()));

        let result = execute(yaml, input, ExecutionConfig::default()).await.unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
    }

    // Note: Parallel execution with LLM nodes is tested in integration tests
    // Unit tests here only cover basic workflow structure
}
