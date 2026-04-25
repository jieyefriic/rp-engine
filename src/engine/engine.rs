//! Engine struct and run method

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;

use crate::error::EngineError;
use crate::nodes::NodeRegistry;
use crate::providers::{ProviderRegistry, EmbeddingRegistry};
use crate::providers::model_registry::ModelRegistry;
use crate::providers::search::SearchProviderRegistry;
use crate::schema::{PreparedWorkflow, Template};

use super::builder::{EngineBuilder, StateSavingConfig};
use crate::state::CheckpointState;
use super::executor::{execute_workflow, ExecutionConfig, ExecutionResult, ResumeParams, ConnectionPools};

/// The main workflow execution engine
///
/// Engine holds all loaded workflows, templates, and configuration.
/// Use `Engine::builder()` to create and configure an instance.
///
/// # Example
/// ```ignore
/// use riceprompt_engine::Engine;
/// use serde_json::json;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let engine = Engine::builder()
///         .workflows_dir("./workflows")
///         .prompts_dir("./prompts")
///         .provider("qwen", std::env::var("QWEN_API_KEY")?)
///         .build()
///         .await?;
///
///     let result = engine.run("chat_agent", json!({
///         "user_input": "Hello!"
///     })).await?;
///
///     println!("Output: {:?}", result.output);
///     Ok(())
/// }
/// ```
pub struct Engine {
    /// Loaded workflows: name -> PreparedWorkflow (Arc for sub-workflow sharing)
    workflows: Arc<HashMap<String, PreparedWorkflow>>,

    /// Loaded templates: template_id -> Template
    templates: HashMap<String, Template>,

    /// Provider registry for LLM calls
    providers: ProviderRegistry,

    /// Embedding provider registry
    embedding_providers: EmbeddingRegistry,

    /// Search provider registry
    search_providers: SearchProviderRegistry,

    /// Custom node registry
    node_registry: Arc<NodeRegistry>,

    /// Connection pools for databases
    pools: Arc<ConnectionPools>,

    /// Model capability registry
    model_registry: Arc<ModelRegistry>,

    /// Transform scripts: script_name -> source code
    transforms: HashMap<String, String>,

    /// MCP server definitions
    mcp_servers: HashMap<String, crate::schema::McpServerDef>,

    /// EvoMap A2A configuration for self-evolution integration
    evomap: Option<super::evomap::EvoMapConfig>,

    /// State saving configuration
    state_saving: Option<StateSavingConfig>,

    /// Default timeout for workflow execution
    timeout: Duration,

    /// Default max iterations for loops
    max_iterations: u32,

    /// Whether to capture actual rendered prompts
    capture_actual_prompt: bool,

    /// Whether to capture raw LLM response
    capture_raw_response: bool,

    /// Whether to embed source YAML in `ExecutionResult.source_yaml`
    capture_source_yaml: bool,
}

impl Engine {
    /// Create a new engine builder
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    /// Create a new engine instance (called by EngineBuilder)
    pub(crate) fn new(
        workflows: HashMap<String, PreparedWorkflow>,
        templates: HashMap<String, Template>,
        transforms: HashMap<String, String>,
        providers: ProviderRegistry,
        embedding_providers: EmbeddingRegistry,
        search_providers: SearchProviderRegistry,
        mcp_servers: HashMap<String, crate::schema::McpServerDef>,
        node_registry: Arc<NodeRegistry>,
        pools: Arc<ConnectionPools>,
        model_registry: Arc<ModelRegistry>,
        state_saving: Option<StateSavingConfig>,
        capture_actual_prompt: bool,
        capture_raw_response: bool,
        capture_source_yaml: bool,
        evomap: Option<super::evomap::EvoMapConfig>,
    ) -> Self {
        Self {
            workflows: Arc::new(workflows),
            templates,
            providers,
            embedding_providers,
            search_providers,
            mcp_servers,
            evomap,
            node_registry,
            pools,
            model_registry,
            transforms,
            state_saving,
            timeout: Duration::from_secs(300), // 5 minutes default
            max_iterations: 100,
            capture_actual_prompt,
            capture_raw_response,
            capture_source_yaml,
        }
    }

    /// Run a workflow by name with the given input
    ///
    /// # Arguments
    /// * `workflow_name` - Name of the workflow (file name without .yaml extension)
    /// * `input` - Input data as a JSON value (will be converted to HashMap)
    ///
    /// # Returns
    /// * `ExecutionResult` containing the output and execution history
    pub async fn run(
        &self,
        workflow_name: &str,
        input: Value,
    ) -> Result<ExecutionResult, EngineError> {
        self.run_internal(workflow_name, input, None).await
    }

    /// Run a workflow with a stream handler for real-time text chunk delivery
    ///
    /// Generate nodes with `stream: true` in the workflow will call
    /// `provider.stream()` and forward text chunks to the handler.
    ///
    /// # Arguments
    /// * `workflow_name` - Name of the workflow
    /// * `input` - Input data as a JSON value
    /// * `stream_handler` - Handler that receives text chunks during streaming
    pub async fn run_with_stream_handler(
        &self,
        workflow_name: &str,
        input: Value,
        stream_handler: Arc<dyn super::StreamHandler>,
    ) -> Result<ExecutionResult, EngineError> {
        self.run_internal(workflow_name, input, Some(stream_handler)).await
    }

    /// Internal implementation shared by run() and run_with_stream_handler()
    async fn run_internal(
        &self,
        workflow_name: &str,
        input: Value,
        stream_handler: Option<Arc<dyn super::StreamHandler>>,
    ) -> Result<ExecutionResult, EngineError> {
        // Get workflow
        let workflow = self.workflows.get(workflow_name)
            .ok_or_else(|| EngineError::InvalidWorkflow(
                format!("Workflow '{}' not found. Available: {:?}", workflow_name, self.list_workflows())
            ))?;

        // Convert input to HashMap
        let input_map = match input {
            Value::Object(map) => {
                map.into_iter().collect::<HashMap<String, Value>>()
            }
            _ => {
                // Wrap non-object input in a map
                let mut map = HashMap::new();
                map.insert("input".to_string(), input);
                map
            }
        };

        // Build execution config
        let mut config = ExecutionConfig::default()
            .with_providers(self.providers.clone())
            .with_embedding_providers(self.embedding_providers.clone())
            .with_search_providers(self.search_providers.clone())
            .with_mcp_servers(self.mcp_servers.clone())
            .with_templates(self.templates.clone())
            .with_node_registry(self.node_registry.clone())
            .with_pools(self.pools.clone())
            .with_model_registry(self.model_registry.clone())
            .with_timeout(self.timeout)
            .with_max_iterations(self.max_iterations)
            .with_capture_actual_prompt(self.capture_actual_prompt)
            .with_capture_raw_response(self.capture_raw_response)
            .with_capture_source_yaml(self.capture_source_yaml)
            .with_workflows(self.workflows.clone())
            .with_transforms(self.transforms.clone());

        // Inject EvoMap config if present
        if let Some(ref evomap_cfg) = self.evomap {
            config = config.with_evomap(evomap_cfg.clone());
        }

        // Inject stream handler if provided
        if let Some(handler) = stream_handler {
            config = config.with_stream_handler(handler);
        }

        // Merge workflow-specific data sources
        for (id, ds) in &workflow.data_sources {
            config.data_sources.insert(id.clone(), ds.clone());
        }

        // Merge workflow-specific providers (override engine-level)
        for (id, provider_def) in &workflow.providers {
            let provider = crate::providers::create_provider_from_def(id, provider_def)?;
            config.providers.register(id, provider);
        }

        // Execute workflow
        let result = execute_workflow(workflow, input_map.clone(), config, None).await?;

        // Save state if enabled
        if let Some(saving_config) = &self.state_saving {
            if saving_config.enabled {
                self.save_state(workflow_name, &input_map, &result, saving_config).await?;
            }
        }

        Ok(result)
    }

    /// Resume a workflow from a checkpoint
    ///
    /// # Arguments
    /// * `workflow_name` - Name of the workflow to resume
    /// * `checkpoint_state` - Serialized state from a previous Paused execution
    /// * `resume_from` - Node ID to resume execution from (this node and all downstream re-execute)
    /// * `new_input` - New input to merge (e.g., user's answer to a question)
    ///
    /// # Returns
    /// * `ExecutionResult` — may be `Paused` again if another checkpoint is hit, or `Success` on completion
    pub async fn resume(
        &self,
        workflow_name: &str,
        checkpoint_state: CheckpointState,
        resume_from: &str,
        new_input: Value,
    ) -> Result<ExecutionResult, EngineError> {
        let workflow = self.workflows.get(workflow_name)
            .ok_or_else(|| EngineError::InvalidWorkflow(
                format!("Workflow '{}' not found. Available: {:?}", workflow_name, self.list_workflows())
            ))?;

        let input_map = match new_input {
            Value::Object(map) => {
                map.into_iter().collect::<HashMap<String, Value>>()
            }
            _ => {
                let mut map = HashMap::new();
                map.insert("input".to_string(), new_input);
                map
            }
        };

        let mut config = ExecutionConfig::default()
            .with_providers(self.providers.clone())
            .with_embedding_providers(self.embedding_providers.clone())
            .with_search_providers(self.search_providers.clone())
            .with_mcp_servers(self.mcp_servers.clone())
            .with_templates(self.templates.clone())
            .with_node_registry(self.node_registry.clone())
            .with_pools(self.pools.clone())
            .with_model_registry(self.model_registry.clone())
            .with_timeout(self.timeout)
            .with_max_iterations(self.max_iterations)
            .with_capture_actual_prompt(self.capture_actual_prompt)
            .with_capture_raw_response(self.capture_raw_response)
            .with_capture_source_yaml(self.capture_source_yaml)
            .with_workflows(self.workflows.clone())
            .with_transforms(self.transforms.clone());

        // Inject EvoMap config if present
        if let Some(ref evomap_cfg) = self.evomap {
            config = config.with_evomap(evomap_cfg.clone());
        }

        for (id, ds) in &workflow.data_sources {
            config.data_sources.insert(id.clone(), ds.clone());
        }

        // Merge workflow-specific providers (override engine-level)
        for (id, provider_def) in &workflow.providers {
            let provider = crate::providers::create_provider_from_def(id, provider_def)?;
            config.providers.register(id, provider);
        }

        let resume_params = ResumeParams {
            checkpoint_state,
            resume_from: resume_from.to_string(),
        };

        let result = execute_workflow(workflow, input_map.clone(), config, Some(resume_params)).await?;

        // Save state if enabled
        if let Some(saving_config) = &self.state_saving {
            if saving_config.enabled {
                self.save_state(workflow_name, &input_map, &result, saving_config).await?;
            }
        }

        Ok(result)
    }

    /// Run a workflow with raw YAML string (for dynamic workflows)
    pub async fn run_yaml(
        &self,
        yaml: &str,
        input: Value,
    ) -> Result<ExecutionResult, EngineError> {
        self.run_yaml_internal(yaml, input, None).await
    }

    /// Run a workflow with raw YAML string and a stream handler
    pub async fn run_yaml_with_stream_handler(
        &self,
        yaml: &str,
        input: Value,
        stream_handler: Arc<dyn super::StreamHandler>,
    ) -> Result<ExecutionResult, EngineError> {
        self.run_yaml_internal(yaml, input, Some(stream_handler)).await
    }

    /// Internal implementation for run_yaml variants
    async fn run_yaml_internal(
        &self,
        yaml: &str,
        input: Value,
        stream_handler: Option<Arc<dyn super::StreamHandler>>,
    ) -> Result<ExecutionResult, EngineError> {
        let workflow = PreparedWorkflow::from_yaml(yaml)
            .map_err(|e| EngineError::InvalidWorkflow(e.to_string()))?;

        // Convert input to HashMap
        let input_map = match input {
            Value::Object(map) => {
                map.into_iter().collect::<HashMap<String, Value>>()
            }
            _ => {
                let mut map = HashMap::new();
                map.insert("input".to_string(), input);
                map
            }
        };

        // Build execution config
        let mut config = ExecutionConfig::default()
            .with_providers(self.providers.clone())
            .with_embedding_providers(self.embedding_providers.clone())
            .with_search_providers(self.search_providers.clone())
            .with_mcp_servers(self.mcp_servers.clone())
            .with_templates(self.templates.clone())
            .with_node_registry(self.node_registry.clone())
            .with_pools(self.pools.clone())
            .with_model_registry(self.model_registry.clone())
            .with_timeout(self.timeout)
            .with_max_iterations(self.max_iterations)
            .with_capture_actual_prompt(self.capture_actual_prompt)
            .with_capture_raw_response(self.capture_raw_response)
            .with_capture_source_yaml(self.capture_source_yaml)
            .with_workflows(self.workflows.clone())
            .with_transforms(self.transforms.clone());

        // Inject EvoMap config if present
        if let Some(ref evomap_cfg) = self.evomap {
            config = config.with_evomap(evomap_cfg.clone());
        }

        // Inject stream handler if provided
        if let Some(handler) = stream_handler {
            config = config.with_stream_handler(handler);
        }

        // Merge workflow-specific data sources
        for (id, ds) in &workflow.data_sources {
            config.data_sources.insert(id.clone(), ds.clone());
        }

        // Merge workflow-specific providers (override engine-level)
        for (id, provider_def) in &workflow.providers {
            let provider = crate::providers::create_provider_from_def(id, provider_def)?;
            config.providers.register(id, provider);
        }

        let mut result = execute_workflow(&workflow, input_map, config, None).await?;

        // Embed the source YAML into the result so a single JSON file is
        // self-describing for offline inspection. We attach the input as
        // received (pre-injection of any external keys by upstream callers)
        // so the snapshot stays user-meaningful and free of secrets.
        if self.capture_source_yaml {
            result.source_yaml = Some(yaml.to_string());
        }
        Ok(result)
    }

    /// Check if a workflow exists
    pub fn has_workflow(&self, name: &str) -> bool {
        self.workflows.contains_key(name)
    }

    /// List all loaded workflow names
    pub fn list_workflows(&self) -> Vec<&str> {
        self.workflows.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a template exists
    pub fn has_template(&self, id: &str) -> bool {
        self.templates.contains_key(id)
    }

    /// List all loaded template IDs
    pub fn list_templates(&self) -> Vec<&str> {
        self.templates.keys().map(|s| s.as_str()).collect()
    }

    /// Get registered custom node types
    pub fn custom_node_types(&self) -> Vec<&str> {
        self.node_registry.node_types()
    }

    /// Set the default timeout for workflow execution
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Set the default max iterations for loops
    pub fn set_max_iterations(&mut self, max: u32) {
        self.max_iterations = max;
    }

    /// Save execution state to file
    async fn save_state(
        &self,
        workflow_name: &str,
        input: &HashMap<String, Value>,
        result: &ExecutionResult,
        config: &StateSavingConfig,
    ) -> Result<(), EngineError> {
        // Ensure directory exists
        if !config.path.exists() {
            std::fs::create_dir_all(&config.path)
                .map_err(|e| EngineError::StateError(format!("Failed to create state directory: {}", e)))?;
        }

        // Build state document
        let state_doc = serde_json::json!({
            "workflow": workflow_name,
            "timestamp": Utc::now().to_rfc3339(),
            "input": input,
            "executions": result.executions,
            "output": result.output,
            "timing": result.timing,
            "token_usage": result.token_usage,
            "status": format!("{:?}", result.status),
            "error": result.error,
        });

        // Generate filename
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let filename = format!("{}_{}.json", workflow_name, timestamp);
        let filepath = config.path.join(filename);

        // Write to file
        let content = serde_json::to_string_pretty(&state_doc)
            .map_err(|e| EngineError::StateError(format!("Failed to serialize state: {}", e)))?;

        std::fs::write(&filepath, content)
            .map_err(|e| EngineError::StateError(format!("Failed to write state file: {}", e)))?;

        tracing::debug!(path = %filepath.display(), "Saved execution state");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_run_not_found() {
        let engine = Engine::builder().build().await.unwrap();
        let result = engine.run("nonexistent", serde_json::json!({})).await;
        assert!(result.is_err());
    }
}
