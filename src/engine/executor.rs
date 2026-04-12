//! Main execution engine with input-based dependency execution

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use deadpool_postgres::{Pool as PgPool, Config as PgConfig, Runtime};
use tracing::{debug, info, warn, error, instrument};

use crate::error::EngineError;
use crate::schema::{PreparedWorkflow, Template, YamlNode, NodeType, ExitReason, DataSourceDef};
use crate::state::{ExecutionState, ExecutionRecord, CheckpointState};
use crate::expression::evaluate_expression;
use crate::providers::{ProviderRegistry, EmbeddingRegistry};
use crate::providers::model_registry::ModelRegistry;
use crate::providers::search::SearchProviderRegistry;
use crate::nodes::{create_executor, NodeRegistry};
use super::graph::ExecutionGraph;

/// Execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    Success,
    Error,
    Timeout,
    Paused,
}

/// Timing information for execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionTiming {
    /// Total execution time in milliseconds
    pub total_ms: u64,
    /// Time per node: node_id -> milliseconds
    pub per_node: HashMap<String, u64>,
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
    /// Redis connection managers keyed by data source ID
    pub redis: HashMap<String, redis::aio::ConnectionManager>,
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
            .map_err(|e| {
                let host = pg_cfg.host.as_deref().unwrap_or("(from url)");
                let dbname = pg_cfg.dbname.as_deref().unwrap_or("(from url)");
                error!(
                    host = %host,
                    database = %dbname,
                    error = %e,
                    "Failed to create PostgreSQL connection pool"
                );
                EngineError::DatabaseError(format!(
                    "Failed to create pool for {}@{}: {}",
                    dbname, host, e
                ))
            })?;

        debug!(
            host = pg_cfg.host.as_deref().unwrap_or("(from url)"),
            database = pg_cfg.dbname.as_deref().unwrap_or("(from url)"),
            max_size = pool_config.max_size,
            "PostgreSQL connection pool created"
        );

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

    /// Get a Redis connection manager by data source ID
    pub fn get_redis_pool(&self, data_source_id: &str) -> Option<&redis::aio::ConnectionManager> {
        self.redis.get(data_source_id)
    }

    /// Register a Redis connection manager for a data source
    pub fn register_redis_pool(&mut self, data_source_id: String, manager: redis::aio::ConnectionManager) {
        self.redis.insert(data_source_id, manager);
    }
}

/// Execution configuration
///
/// Groups three levels of concern:
/// - **Engine-level**: Shared across all workflow runs (providers, pools, registries)
/// - **Workflow-level**: Per-workflow definitions (templates, subgraphs, data_sources)
/// - **Run-level**: Per-execution options (timeout, streaming, observability)
#[derive(Clone)]
pub struct ExecutionConfig {
    // ── Engine-level (shared across all runs) ──────────────────────────

    /// Provider registry for LLM calls
    pub providers: ProviderRegistry,

    /// Embedding provider registry for embedding calls
    pub embedding_providers: EmbeddingRegistry,

    /// Search provider registry for web search calls
    pub search_providers: SearchProviderRegistry,

    /// Custom node registry for user-defined node types
    pub node_registry: Arc<NodeRegistry>,

    /// Connection pools for databases
    pub pools: Arc<ConnectionPools>,

    /// All loaded workflows (for sub-workflow execution)
    pub workflows: Arc<HashMap<String, PreparedWorkflow>>,

    /// Model capability registry (for vision support checks, etc.)
    pub model_registry: Arc<ModelRegistry>,

    /// Transform scripts: script_name -> source code (loaded from transforms_dir)
    pub transforms: HashMap<String, String>,

    // ── Workflow-level (per workflow) ───────────────────────────────────

    /// Templates from the workflow (passed separately for ownership)
    pub templates: HashMap<String, Template>,

    /// Subgraphs (ToolBoxes) from the workflow
    pub subgraphs: HashMap<String, crate::schema::Subgraph>,

    /// Data Sources from the workflow (data_source_id -> DataSourceDef)
    pub data_sources: HashMap<String, DataSourceDef>,

    // ── Run-level options (per execution) ──────────────────────────────

    /// Global timeout
    pub timeout: Duration,

    /// Maximum iterations for any edge (global limit)
    pub max_iterations: u32,

    /// Whether to capture the actual rendered prompts (for observability)
    pub capture_actual_prompt: bool,

    /// Whether to capture the raw LLM response before parsing (for observability)
    pub capture_raw_response: bool,

    /// Optional stream handler for generate nodes with stream: true
    pub stream_handler: Option<Arc<dyn super::StreamHandler>>,

    // ── Harness (workflow-level) ──────────────────────────────────────

    /// Resolved harness instructions text (concatenated from inline + files + memory).
    /// Pre-resolved during execute_workflow setup, so generate nodes just read it.
    pub harness_instructions: Option<String>,

    /// Harness memory file path (for post-execution update)
    pub harness_memory_file: Option<String>,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            providers: ProviderRegistry::new(),
            embedding_providers: EmbeddingRegistry::new(),
            search_providers: SearchProviderRegistry::new(),
            timeout: Duration::from_secs(300), // 5 minutes
            max_iterations: 100,
            templates: HashMap::new(),
            subgraphs: HashMap::new(),
            data_sources: HashMap::new(),
            node_registry: Arc::new(NodeRegistry::new()),
            pools: Arc::new(ConnectionPools::new()),
            capture_actual_prompt: false,
            capture_raw_response: false,
            workflows: Arc::new(HashMap::new()),
            model_registry: Arc::new(ModelRegistry::from_embedded()),
            transforms: HashMap::new(),
            stream_handler: None,
            harness_instructions: None,
            harness_memory_file: None,
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

    /// Set search providers
    pub fn with_search_providers(mut self, search_providers: SearchProviderRegistry) -> Self {
        self.search_providers = search_providers;
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

    /// Set workflows for sub-workflow execution
    pub fn with_workflows(mut self, workflows: Arc<HashMap<String, PreparedWorkflow>>) -> Self {
        self.workflows = workflows;
        self
    }

    /// Get a workflow by name (for sub-workflow execution)
    pub fn get_workflow(&self, name: &str) -> Option<&PreparedWorkflow> {
        self.workflows.get(name)
    }

    /// Set transform scripts (script_name -> source code)
    pub fn with_transforms(mut self, transforms: HashMap<String, String>) -> Self {
        self.transforms = transforms;
        self
    }

    /// Set stream handler for generate nodes with stream: true
    pub fn with_stream_handler(mut self, handler: Arc<dyn super::StreamHandler>) -> Self {
        self.stream_handler = Some(handler);
        self
    }

    /// Set model capability registry
    pub fn with_model_registry(mut self, registry: Arc<ModelRegistry>) -> Self {
        self.model_registry = registry;
        self
    }

    /// Set harness instructions (pre-resolved text)
    pub fn with_harness_instructions(mut self, instructions: String) -> Self {
        self.harness_instructions = Some(instructions);
        self
    }

    /// Set harness memory file path
    pub fn with_harness_memory_file(mut self, path: String) -> Self {
        self.harness_memory_file = Some(path);
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

    /// Checkpoint state for pause/resume (present when status is Paused)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_state: Option<CheckpointState>,
}

/// Parameters for resuming a workflow from a checkpoint
#[derive(Debug, Clone)]
pub struct ResumeParams {
    /// Previous checkpoint state
    pub checkpoint_state: CheckpointState,
    /// Node ID to resume execution from
    pub resume_from: String,
}

/// Result of a single node execution
struct NodeResult {
    node_id: String,
    outputs: HashMap<String, Value>,
    timing_ms: u64,
}


/// Execute a parsed workflow with input-based dependency execution
///
/// Nodes are executed as soon as their input dependencies are satisfied,
/// enabling fine-grained parallelism without layer-based constraints.
///
/// If `checkpoint` is provided, the workflow resumes from the specified node
/// with the previous execution state restored and new input merged.
#[instrument(
    name = "execute_workflow",
    skip(workflow, input, config, checkpoint),
    fields(
        workflow_name = %workflow.name,
        workflow_version = %workflow.version,
        node_count = workflow.nodes.len(),
        edge_count = workflow.edges.len(),
    )
)]
pub async fn execute_workflow(
    workflow: &PreparedWorkflow,
    input: HashMap<String, Value>,
    mut config: ExecutionConfig,
    checkpoint: Option<ResumeParams>,
) -> Result<ExecutionResult, EngineError> {
    let start_time = Instant::now();

    info!(
        workflow = %workflow.name,
        input_keys = ?input.keys().collect::<Vec<_>>(),
        "Starting workflow execution"
    );

    // Validate input against workflow schema (skip on resume — input is merged)
    if checkpoint.is_none() {
        if let Err(e) = workflow.input.validate(&input) {
            error!(
                workflow = %workflow.name,
                error = %e,
                expected_schema = ?workflow.input,
                provided_keys = ?input.keys().collect::<Vec<_>>(),
                "Input validation failed"
            );
            return Err(e);
        }
    }

    // Merge workflow templates into config
    for (id, template) in &workflow.templates {
        config.templates.insert(id.clone(), template.clone());
    }

    // Merge workflow transforms into config
    for (name, source) in &workflow.transforms {
        config.transforms.insert(name.clone(), source.clone());
    }

    // Merge workflow subgraphs into config
    for (id, subgraph) in &workflow.subgraphs {
        config.subgraphs.insert(id.clone(), subgraph.clone());
    }

    // Merge workflow data sources into config
    for (id, data_source) in &workflow.data_sources {
        config.data_sources.insert(id.clone(), data_source.clone());
    }

    // Resolve harness instructions (inline text + external files + memory)
    if let Some(ref harness) = workflow.harness {
        let mut parts: Vec<String> = Vec::new();

        // 1. Inline instructions
        if let Some(ref text) = harness.instructions {
            parts.push(text.clone());
        }

        // 2. External files
        for file_path in &harness.files {
            match std::fs::read_to_string(file_path) {
                Ok(content) => parts.push(content),
                Err(e) => tracing::warn!(file = %file_path, error = %e, "Failed to read harness instruction file"),
            }
        }

        // 3. Memory file content (if exists)
        if let Some(ref memory_path) = harness.memory_file {
            if let Ok(content) = std::fs::read_to_string(memory_path) {
                if !content.trim().is_empty() {
                    parts.push(format!("## Memory\n{}", content));
                }
            }
            config.harness_memory_file = Some(memory_path.clone());
        }

        if !parts.is_empty() {
            config.harness_instructions = Some(parts.join("\n\n---\n\n"));
        }
    }

    // Extract writeback config before Arc wrapping (needed for post-execution)
    let writeback_config = workflow.harness.as_ref()
        .and_then(|h| h.writeback.clone());
    let writeback_memory_path = config.harness_memory_file.clone();

    // Wrap config in Arc for sharing across tasks
    let config = Arc::new(config);

    let graph = ExecutionGraph::from_workflow(workflow);

    // Initialize state and tracking sets — either fresh or from checkpoint
    let (state, mut executed, mut pending_nodes) = if let Some(ref resume) = checkpoint {
        info!(
            workflow = %workflow.name,
            resume_from = %resume.resume_from,
            checkpoint_nodes = ?resume.checkpoint_state.executions.keys().collect::<Vec<_>>(),
            "Resuming workflow from checkpoint"
        );

        let restored = ExecutionState::from_checkpoint(&resume.checkpoint_state, input);

        // Compute forward-reachable set from resume point
        let reachable = compute_forward_reachable(&graph, &resume.resume_from);

        let start_id = workflow.find_start_node()
            .map(|n| n.id.clone())
            .unwrap_or_default();

        let mut pre_executed: HashSet<String> = HashSet::new();
        let mut pending: HashSet<String> = HashSet::new();

        for node in &workflow.nodes {
            if reachable.contains(&node.id) || node.id == start_id {
                // Resume point + downstream + start → re-execute
                pending.insert(node.id.clone());
            } else if restored.has_node_output(&node.id) {
                // Upstream nodes with existing outputs → keep as executed
                pre_executed.insert(node.id.clone());
            } else {
                // Never-executed nodes not in reachable set → keep pending
                pending.insert(node.id.clone());
            }
        }

        (Arc::new(RwLock::new(restored)), pre_executed, pending)
    } else {
        let state = ExecutionState::new(input);
        let pending: HashSet<String> = workflow.nodes.iter()
            .map(|n| n.id.clone())
            .collect();
        (Arc::new(RwLock::new(state)), HashSet::new(), pending)
    };

    let mut in_flight: HashSet<String> = HashSet::new();
    let mut timing = ExecutionTiming::default();
    let mut token_usage = TokenUsage::default();
    let mut final_output: HashMap<String, Value> = HashMap::new();

    // Main execution loop - input-based dependency checking
    loop {
        // Check timeout
        if start_time.elapsed() > config.timeout {
            let elapsed_ms = start_time.elapsed().as_millis();
            error!(
                workflow = %workflow.name,
                elapsed_ms = elapsed_ms,
                timeout_ms = config.timeout.as_millis(),
                pending_nodes = ?pending_nodes,
                in_flight_nodes = ?in_flight,
                executed_nodes = ?executed,
                "Workflow execution timeout"
            );
            let state_guard = state.read().await;
            return Ok(ExecutionResult {
                status: ExecutionStatus::Timeout,
                executions: state_guard.get_executions().clone(),
                output: final_output,
                error: Some(format!(
                    "Execution timeout after {}ms (limit: {}ms). Pending: {:?}, In-flight: {:?}",
                    elapsed_ms, config.timeout.as_millis(), pending_nodes, in_flight
                )),
                timing,
                token_usage,
                exit_reason: None,
                checkpoint_state: None,
            });
        }

        // Find all ready nodes (inputs satisfied, not executed, not in-flight)
        let ready_nodes: Vec<String> = {
            let state_guard = state.read().await;
            pending_nodes.iter()
                .filter(|node_id| {
                    !executed.contains(*node_id)
                        && !in_flight.contains(*node_id)
                        && graph.inputs_satisfied(node_id, &state_guard)
                })
                .cloned()
                .collect()
        };

        // Check if we're done or stuck
        if ready_nodes.is_empty() && in_flight.is_empty() {
            // No more nodes to execute
            if pending_nodes.is_empty() {
                break; // Successfully completed
            } else {
                // Stuck - some nodes never became ready
                warn!(
                    workflow = %workflow.name,
                    stuck_nodes = ?pending_nodes,
                    "Some nodes never became ready, completing with partial execution"
                );
                break;
            }
        }

        if ready_nodes.is_empty() {
            // Wait for in-flight nodes to complete
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        }

        debug!(
            workflow = %workflow.name,
            ready_nodes = ?ready_nodes,
            in_flight = in_flight.len(),
            pending = pending_nodes.len(),
            "Executing ready nodes"
        );

        // Mark nodes as in-flight
        for node_id in &ready_nodes {
            in_flight.insert(node_id.clone());
            pending_nodes.remove(node_id);
        }

        // Execute all ready nodes in PARALLEL
        let results = match execute_nodes_parallel(
            &ready_nodes,
            &graph,
            state.clone(),
            config.clone(),
            &workflow.name,
        ).await {
            Ok(r) => r,
            Err(e) => {
                error!(
                    workflow = %workflow.name,
                    nodes = ?ready_nodes,
                    error = %e,
                    "Parallel execution failed"
                );
                return Err(e);
            }
        };

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
            debug!(
                workflow = %workflow.name,
                node = %result.node_id,
                short_id = %short_id,
                timing_ms = result.timing_ms,
                "Node state recorded"
            );

            // If this is a checkpoint node, pause execution and return state
            if graph.is_checkpoint_node(&result.node_id) {
                let cp_state = state_guard.to_checkpoint(result.outputs.clone());
                timing.total_ms = start_time.elapsed().as_millis() as u64;
                info!(
                    workflow = %workflow.name,
                    checkpoint_node = %result.node_id,
                    "Workflow paused at checkpoint"
                );
                return Ok(ExecutionResult {
                    status: ExecutionStatus::Paused,
                    executions: state_guard.get_executions().clone(),
                    output: result.outputs,
                    error: None,
                    timing,
                    token_usage,
                    exit_reason: None,
                    checkpoint_state: Some(cp_state),
                });
            }

            // If this is a response node, capture final output
            if graph.is_response_node(&result.node_id) {
                final_output = result.outputs;
            }

            // Mark as executed, remove from in-flight
            executed.insert(result.node_id.clone());
            in_flight.remove(&result.node_id);

            // Evaluate outgoing edges and add next nodes to pending
            let next_nodes = evaluate_edges(&graph, &result.node_id, &mut state_guard, &config, exit_reason)?;
            for next in next_nodes {
                if !executed.contains(&next) && !in_flight.contains(&next) {
                    pending_nodes.insert(next);
                }
            }
        }

        // Propagate dead paths: mark nodes whose incoming edges are all inactive
        propagate_dead_paths(&graph, &mut state_guard);

        // Remove dead nodes from pending (they will never execute)
        let newly_dead: Vec<String> = pending_nodes.iter()
            .filter(|n| state_guard.is_dead(n))
            .cloned()
            .collect();
        for dead_id in &newly_dead {
            pending_nodes.remove(dead_id);
            debug!(node = %dead_id, "Removed dead node from pending set");
        }

        drop(state_guard);
    }

    timing.total_ms = start_time.elapsed().as_millis() as u64;

    info!(
        workflow = %workflow.name,
        total_ms = timing.total_ms,
        total_input_tokens = token_usage.total_input_tokens,
        total_output_tokens = token_usage.total_output_tokens,
        executed_nodes = executed.len(),
        "Workflow execution completed successfully"
    );

    // Harness writeback: post-execution memory update via dedicated LLM call
    if let (Some(wb), Some(mem_path)) = (&writeback_config, &writeback_memory_path) {
        info!(
            workflow = %workflow.name,
            provider = %wb.provider,
            model = %wb.model,
            memory_file = %mem_path,
            "Starting harness writeback"
        );

        match harness_writeback(wb, mem_path, &final_output, &config).await {
            Ok(()) => {
                info!(workflow = %workflow.name, "Harness writeback completed");
            }
            Err(e) => {
                // Writeback is best-effort — don't fail the workflow
                warn!(
                    workflow = %workflow.name,
                    error = %e,
                    "Harness writeback failed (non-fatal)"
                );
            }
        }
    }

    let state_guard = state.read().await;
    Ok(ExecutionResult {
        status: ExecutionStatus::Success,
        executions: state_guard.get_executions().clone(),
        output: final_output,
        error: None,
        timing,
        token_usage,
        exit_reason: None,
        checkpoint_state: None,
    })
}

/// Execute harness post-execution writeback: a dedicated LLM call reviews the workflow output
/// and current memory, then produces updated memory content written to memory_file.
async fn harness_writeback(
    wb: &crate::schema::WritebackConfig,
    memory_path: &str,
    workflow_output: &HashMap<String, Value>,
    config: &ExecutionConfig,
) -> Result<(), EngineError> {
    use crate::providers::{ChatOptions, Message};

    // Get the writeback provider
    let provider = config.providers.get(&wb.provider)
        .ok_or_else(|| {
            error!(provider = %wb.provider, "Writeback provider not found");
            EngineError::ProviderNotFound(wb.provider.clone())
        })?;

    // Read current memory (may not exist yet)
    let current_memory = std::fs::read_to_string(memory_path).unwrap_or_default();

    // Build the writeback prompt
    let system_prompt = wb.prompt.clone().unwrap_or_else(|| {
        "You are a memory manager for an AI workflow harness. \
         Your job is to review the workflow execution results and update the persistent memory. \
         Focus on: user preferences, habits, key decisions, and important context for future runs. \
         Be concise. Remove outdated information. \
         Output ONLY the updated memory content — no explanation, no markdown fences.".to_string()
    });

    let char_limit = wb.memory_char_limit.unwrap_or(2200);

    let output_json = serde_json::to_string_pretty(workflow_output)
        .unwrap_or_else(|_| format!("{:?}", workflow_output));

    let user_prompt = format!(
        "## Current Memory ({}/{} chars)\n{}\n\n## Workflow Output\n{}\n\n\
         Based on the above, produce the updated memory content. \
         Stay within {} characters.",
        current_memory.len(), char_limit,
        if current_memory.is_empty() { "(empty)" } else { &current_memory },
        output_json,
        char_limit,
    );

    // Make the writeback LLM call
    let options = ChatOptions::new()
        .with_model(&wb.model)
        .with_temperature(0.3)
        .with_max_tokens(1024);

    let messages = vec![
        Message::system(system_prompt),
        Message::user(user_prompt),
    ];

    let response = provider.chat(messages, options).await
        .map_err(|e| {
            error!(error = %e, "Harness writeback LLM call failed");
            EngineError::ProviderError(e)
        })?;

    // Truncate to char limit
    let mut new_memory = response.content;
    if new_memory.len() > char_limit {
        warn!(
            len = new_memory.len(),
            limit = char_limit,
            "Writeback content exceeds char limit, truncating"
        );
        new_memory.truncate(char_limit);
    }

    // Write back to memory file
    std::fs::write(memory_path, &new_memory)
        .map_err(|e| {
            error!(path = %memory_path, error = %e, "Failed to write memory file");
            EngineError::NodeExecutionFailed(
                format!("Failed to write harness memory file '{}': {}", memory_path, e)
            )
        })?;

    info!(
        memory_path = %memory_path,
        old_len = current_memory.len(),
        new_len = new_memory.len(),
        char_limit = char_limit,
        "Harness memory updated"
    );

    Ok(())
}

/// Compute all nodes reachable by walking forward from a given node via outgoing edges
fn compute_forward_reachable(graph: &ExecutionGraph, start_node: &str) -> HashSet<String> {
    let mut reachable = HashSet::new();
    let mut queue = vec![start_node.to_string()];

    while let Some(node_id) = queue.pop() {
        if !reachable.insert(node_id.clone()) {
            continue;
        }
        for edge in graph.get_outgoing_edges(&node_id) {
            queue.push(edge.to.clone());
        }
    }

    reachable
}

/// Execute multiple nodes in parallel
async fn execute_nodes_parallel(
    node_ids: &[String],
    graph: &ExecutionGraph<'_>,
    state: Arc<RwLock<ExecutionState>>,
    config: Arc<ExecutionConfig>,
    workflow_name: &str,
) -> Result<Vec<NodeResult>, EngineError> {
    use futures::future::join_all;

    // Prepare node data for parallel execution
    let mut tasks = Vec::new();
    for node_id in node_ids {
        let node = match graph.get_node(node_id) {
            Ok(n) => n.clone(),
            Err(e) => return Err(e),
        };
        let state = state.clone();
        let config = config.clone();
        let wf_name = workflow_name.to_string();

        tasks.push(async move {
            execute_single_node(node, state, config, &wf_name).await
        });
    }

    // Execute all in parallel
    let results = join_all(tasks).await;

    // Collect results, fail on first error with detailed logging
    let mut node_results = Vec::new();
    for (i, result) in results.into_iter().enumerate() {
        match result {
            Ok(r) => node_results.push(r),
            Err(e) => {
                let failed_node = &node_ids[i];
                error!(
                    workflow = %workflow_name,
                    node = %failed_node,
                    parallel_batch = ?node_ids,
                    error = %e,
                    "Node execution failed in parallel batch"
                );
                return Err(e);
            }
        }
    }

    Ok(node_results)
}

/// Execute a single node
#[instrument(
    name = "execute_node",
    skip(state, config),
    fields(
        node_id = %node.id,
        node_type = ?node.node_type,
    )
)]
async fn execute_single_node(
    node: YamlNode,
    state: Arc<RwLock<ExecutionState>>,
    config: Arc<ExecutionConfig>,
    workflow_name: &str,
) -> Result<NodeResult, EngineError> {
    let node_start = Instant::now();
    let node_id = node.id.clone();
    let node_type = format!("{:?}", node.node_type);

    debug!(
        workflow = %workflow_name,
        node = %node_id,
        node_type = %node_type,
        "Starting node execution"
    );

    // Get error handling config
    let error_handling = node.config.error_handling.clone();
    let max_retries = error_handling.as_ref().map(|e| e.retry).unwrap_or(0);
    let retry_delay = error_handling.as_ref().map(|e| e.retry_delay_ms).unwrap_or(1000);
    let timeout_ms = error_handling.as_ref().and_then(|e| e.timeout_ms);
    let on_exhausted = error_handling.as_ref()
        .map(|e| e.on_exhausted.as_str())
        .unwrap_or("fail");

    let mut last_error: Option<EngineError> = None;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            info!(
                workflow = %workflow_name,
                node = %node_id,
                attempt = attempt,
                max_retries = max_retries,
                "Retrying node execution"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(retry_delay)).await;
        }

        // Get read-only snapshot of state for execution
        let state_snapshot = {
            let guard = state.read().await;
            guard.clone()
        };

        // Execute the node (with optional timeout)
        let exec_result = execute_node_once(&node, &state_snapshot, &config, workflow_name).await;

        // Apply timeout if configured
        let exec_result = if let Some(ms) = timeout_ms {
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(ms),
                async { exec_result },
            ).await {
                Ok(r) => r,
                Err(_) => Err(EngineError::NodeExecutionFailed(
                    format!("Node '{}': execution timed out after {}ms", node_id, ms)
                )),
            }
        } else {
            exec_result
        };

        match exec_result {
            Ok(outputs) => {
                let timing_ms = node_start.elapsed().as_millis() as u64;
                debug!(
                    workflow = %workflow_name,
                    node = %node_id,
                    timing_ms = timing_ms,
                    output_keys = ?outputs.keys().collect::<Vec<_>>(),
                    "Node execution completed"
                );
                return Ok(NodeResult { node_id, outputs, timing_ms });
            }
            Err(e) => {
                warn!(
                    workflow = %workflow_name,
                    node = %node_id,
                    attempt = attempt,
                    max_retries = max_retries,
                    error = %e,
                    "Node execution failed"
                );
                last_error = Some(e);
            }
        }
    }

    // All retries exhausted
    let err = last_error.unwrap();
    let timing_ms = node_start.elapsed().as_millis() as u64;

    if on_exhausted == "continue" {
        // Return success with error markers — enables on_error edge routing
        let mut outputs = HashMap::new();
        outputs.insert("_error".to_string(), serde_json::Value::Bool(true));
        outputs.insert("_error_message".to_string(), serde_json::Value::String(err.to_string()));
        info!(
            workflow = %workflow_name,
            node = %node_id,
            "Node failed after retries, continuing with error outputs (on_exhausted=continue)"
        );
        Ok(NodeResult { node_id, outputs, timing_ms })
    } else {
        error!(
            workflow = %workflow_name,
            node = %node_id,
            error = %err,
            "Node failed after all retries (on_exhausted=fail)"
        );
        Err(err)
    }
}

/// Execute a node once (no retry/timeout logic)
async fn execute_node_once(
    node: &YamlNode,
    state_snapshot: &ExecutionState,
    config: &Arc<ExecutionConfig>,
    workflow_name: &str,
) -> Result<HashMap<String, serde_json::Value>, EngineError> {
    let node_id = &node.id;
    let node_type = format!("{:?}", node.node_type);

    match &node.node_type {
        NodeType::Custom(type_name) => {
            match config.node_registry.get(type_name) {
                Some(custom_node) => {
                    custom_node.execute(&node.config, state_snapshot, config).await
                        .map_err(|e| {
                            error!(
                                workflow = %workflow_name,
                                node = %node_id,
                                custom_type = %type_name,
                                error = %e,
                                "Custom node execution failed"
                            );
                            e
                        })
                }
                None => {
                    Err(EngineError::NodeExecutionFailed(
                        format!("Node '{}': Custom node type '{}' not found in registry", node_id, type_name)
                    ))
                }
            }
        }
        _ => {
            let executor = create_executor(&node.node_type);
            executor.execute(node, state_snapshot, config).await
                .map_err(|e| {
                    error!(
                        workflow = %workflow_name,
                        node = %node_id,
                        node_type = %node_type,
                        error = %e,
                        "Built-in node execution failed"
                    );
                    e
                })
        }
    }
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
    // 1. on_error: true    → Only taken when source node has _error output
    // 2. is_default: true  → ALWAYS execute (unconditionally), suppressed if on_error active
    // 3. condition: expr   → Execute if expr evaluates to true
    // 4. No condition, not default → Execute unconditionally

    // Check if the source node failed (has _error in outputs)
    let node_has_error = state.get_output(&format!("{}._error", from_node))
        .map(|v| v == serde_json::Value::Bool(true))
        .unwrap_or(false);

    // If node has error, check if there are on_error edges
    let has_on_error_edges = edges.iter().any(|e| e.on_error);

    for edge in &edges {
        // Skip exit edges in regular evaluation
        if edge.exit_reason.is_some() {
            continue;
        }

        // on_error edge routing
        if edge.on_error {
            // Only activate on_error edges when node actually failed
            if !node_has_error {
                continue;
            }
            // on_error edge is active — take it
            next.push(edge.to.clone());
            state.increment_edge_count(&edge.from, &edge.to);
            continue;
        }

        // Normal edges: suppress if node failed AND on_error edges exist
        if node_has_error && has_on_error_edges {
            state.mark_edge_dead(&edge.from, &edge.to);
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
                Ok(result) => {
                    debug!(
                        from = %edge.from,
                        to = %edge.to,
                        condition = %expr,
                        result = result,
                        "Edge condition evaluated"
                    );
                    result
                },
                Err(e) => {
                    warn!(
                        from = %edge.from,
                        to = %edge.to,
                        condition = %expr,
                        error = %e,
                        "Edge condition evaluation failed, treating as false"
                    );
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
        } else if edge.condition.is_some() {
            // Conditional edge evaluated to false — record for dead path detection
            state.mark_edge_dead(&edge.from, &edge.to);
        }
    }

    Ok(next)
}

/// Propagate dead paths through the graph.
///
/// A node is dead when ALL its incoming edges are "inactive":
/// - Source node is dead (will never execute), OR
/// - The edge is a conditional edge that evaluated to false (recorded in dead_edges)
///
/// Repeats until no new dead nodes are found (fixed-point).
fn propagate_dead_paths(
    graph: &ExecutionGraph,
    state: &mut ExecutionState,
) {
    loop {
        let mut changed = false;

        for node_id in graph.all_nodes() {
            // Skip already-dead or already-completed nodes
            if state.is_dead(node_id) || state.has_node_output(node_id) {
                continue;
            }

            let incoming = graph.get_incoming_edges(node_id);

            // Start node (no incoming edges) is never dead
            if incoming.is_empty() {
                continue;
            }

            // Check if ALL incoming edges are inactive
            let all_inactive = incoming.iter().all(|edge| {
                state.is_dead(&edge.from) || state.is_edge_dead(&edge.from, &edge.to)
            });

            if all_inactive {
                debug!(
                    node = %node_id,
                    "Node confirmed unreachable (dead path)"
                );
                state.mark_dead(node_id);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }
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
    config:
      output:
        message: "input.query"
  - id: response
    type: response
edges:
  - from: start
    to: response
"#;
        let workflow = PreparedWorkflow::from_yaml(yaml).unwrap();
        let mut input = HashMap::new();
        input.insert("query".to_string(), Value::String("hello".to_string()));

        let result = execute_workflow(&workflow, input, ExecutionConfig::default(), None).await.unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
    }
}
