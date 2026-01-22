//! Engine struct and run method

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;

use crate::error::EngineError;
use crate::nodes::NodeRegistry;
use crate::providers::{ProviderRegistry, EmbeddingRegistry};
use crate::schema::{PreparedWorkflow, Template};

use super::builder::{EngineBuilder, StateSavingConfig};
use super::executor::{execute_workflow, ExecutionConfig, ExecutionResult, ConnectionPools};

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

    /// Custom node registry
    node_registry: Arc<NodeRegistry>,

    /// Connection pools for databases
    pools: Arc<ConnectionPools>,

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
        providers: ProviderRegistry,
        embedding_providers: EmbeddingRegistry,
        node_registry: Arc<NodeRegistry>,
        pools: Arc<ConnectionPools>,
        state_saving: Option<StateSavingConfig>,
        capture_actual_prompt: bool,
        capture_raw_response: bool,
    ) -> Self {
        Self {
            workflows: Arc::new(workflows),
            templates,
            providers,
            embedding_providers,
            node_registry,
            pools,
            state_saving,
            timeout: Duration::from_secs(300), // 5 minutes default
            max_iterations: 100,
            capture_actual_prompt,
            capture_raw_response,
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
            .with_templates(self.templates.clone())
            .with_node_registry(self.node_registry.clone())
            .with_pools(self.pools.clone())
            .with_timeout(self.timeout)
            .with_max_iterations(self.max_iterations)
            .with_capture_actual_prompt(self.capture_actual_prompt)
            .with_capture_raw_response(self.capture_raw_response)
            .with_workflows(self.workflows.clone());

        // Merge workflow-specific data sources
        for (id, ds) in &workflow.data_sources {
            config.data_sources.insert(id.clone(), ds.clone());
        }

        // Execute workflow
        let result = execute_workflow(workflow, input_map.clone(), config).await?;

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
        let config = ExecutionConfig::default()
            .with_providers(self.providers.clone())
            .with_embedding_providers(self.embedding_providers.clone())
            .with_templates(self.templates.clone())
            .with_node_registry(self.node_registry.clone())
            .with_pools(self.pools.clone())
            .with_timeout(self.timeout)
            .with_max_iterations(self.max_iterations)
            .with_workflows(self.workflows.clone());

        execute_workflow(&workflow, input_map, config).await
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
