//! Engine builder for fluent configuration

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, error};

use crate::error::EngineError;
use crate::nodes::{CustomNode, NodeRegistry, WorkflowNode};
use crate::providers::{
    create_registry_with_keys,
    EmbeddingRegistry,
    DashScopeEmbeddingProvider, OpenAIEmbeddingProvider,
};
use crate::schema::{PreparedWorkflow, Template};

use super::engine::Engine;
use super::executor::{ConnectionPools, PoolConfig};

/// Configuration for state saving
#[derive(Debug, Clone)]
pub struct StateSavingConfig {
    /// Whether state saving is enabled
    pub enabled: bool,
    /// Directory to save state files
    pub path: PathBuf,
}

/// Builder for configuring and creating an Engine instance
///
/// # Example
/// ```ignore
/// use riceprompt_engine::Engine;
/// use std::sync::Arc;
///
/// let engine = Engine::builder()
///     .workflows_dir("./workflows")
///     .prompts_dir("./prompts")
///     .provider("qwen", std::env::var("QWEN_API_KEY")?)
///     .provider("openai", std::env::var("OPENAI_API_KEY")?)
///     .register_node(Arc::new(MyCustomNode::new()))
///     .state_saving(true, "./state")
///     .build()
///     .await?;
/// ```
pub struct EngineBuilder {
    /// Directory containing workflow YAML files
    workflows_dir: Option<PathBuf>,

    /// Directory containing prompt template YAML files
    prompts_dir: Option<PathBuf>,

    /// Directory containing transform Rhai script files
    transforms_dir: Option<PathBuf>,

    /// Provider API keys: provider_name -> api_key
    providers: HashMap<String, String>,

    /// Embedding provider API keys: provider_name -> api_key
    embedding_providers: HashMap<String, String>,

    /// Custom node implementations to register
    custom_nodes: Vec<Arc<dyn CustomNode>>,

    /// State saving configuration
    state_saving: Option<StateSavingConfig>,

    /// Pool configuration for database connections
    pool_config: PoolConfig,

    /// Pre-initialized connection pools
    connection_pools: ConnectionPools,

    /// Whether to capture actual rendered prompts (for observability)
    capture_actual_prompt: bool,

    /// Whether to capture raw LLM response (for observability)
    capture_raw_response: bool,
}

impl Default for EngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineBuilder {
    /// Create a new engine builder with default settings
    pub fn new() -> Self {
        Self {
            workflows_dir: None,
            prompts_dir: None,
            transforms_dir: None,
            providers: HashMap::new(),
            embedding_providers: HashMap::new(),
            custom_nodes: Vec::new(),
            state_saving: None,
            pool_config: PoolConfig::default(),
            connection_pools: ConnectionPools::new(),
            capture_actual_prompt: false,
            capture_raw_response: false,
        }
    }

    /// Set the directory containing workflow YAML files
    ///
    /// All `.yaml` files in this directory will be loaded as workflows.
    /// The file name (without extension) becomes the workflow name.
    pub fn workflows_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.workflows_dir = Some(path.into());
        self
    }

    /// Set the directory containing prompt template YAML files
    ///
    /// All `.yaml` files in this directory will be loaded as templates.
    /// The file name (without extension) becomes the template ID.
    pub fn prompts_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.prompts_dir = Some(path.into());
        self
    }

    /// Set the directory containing transform Rhai script files
    ///
    /// All `.rhai` files in this directory will be loaded as transform scripts.
    /// The file name (without extension) becomes the script name used in
    /// `config.file` of transform nodes.
    pub fn transforms_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.transforms_dir = Some(path.into());
        self
    }

    /// Register a provider with its API key
    ///
    /// Supported providers: openai, anthropic, gemini, deepseek, qwen, xai, zhipu, moonshot
    pub fn provider(mut self, name: &str, api_key: impl Into<String>) -> Self {
        self.providers.insert(name.to_string(), api_key.into());
        self
    }

    /// Register an embedding provider with its API key
    ///
    /// Supported embedding providers: dashscope, openai
    pub fn embedding_provider(mut self, name: &str, api_key: impl Into<String>) -> Self {
        self.embedding_providers.insert(name.to_string(), api_key.into());
        self
    }

    /// Register a custom node implementation
    ///
    /// Custom nodes can be used in workflows with `type: <node_type>`
    /// where `<node_type>` matches the value returned by `CustomNode::node_type()`.
    pub fn register_node(mut self, node: Arc<dyn CustomNode>) -> Self {
        self.custom_nodes.push(node);
        self
    }

    /// Enable state saving to a directory
    ///
    /// When enabled, each workflow execution will save its complete state
    /// to a JSON file in the specified directory.
    pub fn state_saving(mut self, enabled: bool, path: impl Into<PathBuf>) -> Self {
        self.state_saving = Some(StateSavingConfig {
            enabled,
            path: path.into(),
        });
        self
    }

    /// Set the pool configuration for database connections
    ///
    /// This affects all pools created by the engine.
    pub fn pool_config(mut self, config: PoolConfig) -> Self {
        self.pool_config = config;
        self
    }

    /// Add a PostgreSQL connection pool for a data source
    ///
    /// # Arguments
    /// * `data_source_id` - The ID of the data source (used in workflow config)
    /// * `pool` - A pre-configured deadpool-postgres Pool
    ///
    /// # Example
    /// ```ignore
    /// use deadpool_postgres::{Config, Runtime};
    ///
    /// let mut cfg = Config::new();
    /// cfg.host = Some("localhost".to_string());
    /// cfg.dbname = Some("mydb".to_string());
    /// let pool = cfg.create_pool(Some(Runtime::Tokio1), tokio_postgres::NoTls)?;
    ///
    /// Engine::builder()
    ///     .pg_pool("main_db", pool)
    ///     .build()
    ///     .await?;
    /// ```
    pub fn pg_pool(mut self, data_source_id: impl Into<String>, pool: deadpool_postgres::Pool) -> Self {
        self.connection_pools.register_pg_pool(data_source_id.into(), pool);
        self
    }

    /// Enable capturing of actual rendered prompts (for observability)
    ///
    /// When enabled, generate nodes will include `_actual_system_prompt` and
    /// `_actual_user_prompt` in their outputs, containing the fully rendered
    /// prompt text that was sent to the LLM.
    pub fn capture_actual_prompt(mut self, enabled: bool) -> Self {
        self.capture_actual_prompt = enabled;
        self
    }

    /// Enable capturing of raw LLM response (for observability)
    ///
    /// When enabled, generate nodes will include `_raw_response` in their
    /// outputs, containing the raw response text from the LLM before JSON parsing.
    pub fn capture_raw_response(mut self, enabled: bool) -> Self {
        self.capture_raw_response = enabled;
        self
    }

    /// Build the engine with the configured settings
    pub async fn build(self) -> Result<Engine, EngineError> {
        // Build node registry
        let mut node_registry = NodeRegistry::new();

        // Register built-in workflow node for sub-workflow execution
        node_registry.register(Arc::new(WorkflowNode::new()));

        // Register user-provided custom nodes
        for node in self.custom_nodes {
            node_registry.register(node);
        }

        // Build provider registry by extracting individual keys
        let provider_registry = create_registry_with_keys(
            self.providers.get("openai").cloned(),
            self.providers.get("anthropic").cloned(),
            self.providers.get("gemini").cloned(),
            self.providers.get("deepseek").cloned(),
            self.providers.get("xai").cloned(),
            self.providers.get("qwen").cloned(),
            self.providers.get("zhipu").cloned(),
            self.providers.get("moonshot").cloned(),
        );

        // Build embedding registry
        let mut embedding_registry = EmbeddingRegistry::new();
        if let Some(key) = self.embedding_providers.get("dashscope") {
            embedding_registry.register("dashscope", Arc::new(DashScopeEmbeddingProvider::new(key.clone())));
        }
        if let Some(key) = self.embedding_providers.get("openai") {
            embedding_registry.register("openai", Arc::new(OpenAIEmbeddingProvider::new(key.clone())));
        }

        // Load workflows from directory
        let workflows = if let Some(dir) = &self.workflows_dir {
            load_workflows(dir).await?
        } else {
            HashMap::new()
        };

        // Load global templates from prompts directory
        // These are merged with workflow.templates at runtime in Engine.run()
        let templates = if let Some(dir) = &self.prompts_dir {
            load_prompts(dir).await?
        } else {
            HashMap::new()
        };

        // Load transform scripts from transforms directory
        let transforms = if let Some(dir) = &self.transforms_dir {
            load_transforms(dir)?
        } else {
            HashMap::new()
        };

        Ok(Engine::new(
            workflows,
            templates,
            transforms,
            provider_registry,
            embedding_registry,
            Arc::new(node_registry),
            Arc::new(self.connection_pools),
            self.state_saving,
            self.capture_actual_prompt,
            self.capture_raw_response,
        ))
    }
}

/// Load all workflow YAML files from a directory
/// Replace ${VAR_NAME} patterns with environment variable values
fn expand_env_vars(content: &str) -> String {
    let re = regex_lite::Regex::new(r"\$\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();

    re.replace_all(content, |caps: &regex_lite::Captures| {
        let var_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        std::env::var(var_name).unwrap_or_else(|_| {
            tracing::warn!(var = %var_name, "Environment variable not found, using empty string");
            String::new()
        })
    }).to_string()
}

async fn load_workflows(dir: &PathBuf) -> Result<HashMap<String, PreparedWorkflow>, EngineError> {
    let mut workflows = HashMap::new();

    debug!(directory = %dir.display(), "Loading workflows from directory");

    if !dir.exists() {
        error!(directory = %dir.display(), "Workflows directory does not exist");
        return Err(EngineError::InvalidWorkflow(
            format!("Workflows directory does not exist: {}", dir.display())
        ));
    }

    let entries = std::fs::read_dir(dir)
        .map_err(|e| {
            error!(directory = %dir.display(), error = %e, "Failed to read workflows directory");
            EngineError::InvalidWorkflow(format!("Failed to read workflows directory {}: {}", dir.display(), e))
        })?;

    for entry in entries {
        let entry = entry
            .map_err(|e| {
                error!(directory = %dir.display(), error = %e, "Failed to read directory entry");
                EngineError::InvalidWorkflow(format!("Failed to read directory entry: {}", e))
            })?;
        let path = entry.path();

        // Only process .yaml files
        if path.extension().map(|e| e == "yaml" || e == "yml").unwrap_or(false) {
            let name = path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    error!(path = %path.display(), "Invalid workflow file name");
                    EngineError::InvalidWorkflow(format!("Invalid workflow file name: {}", path.display()))
                })?
                .to_string();

            let content = std::fs::read_to_string(&path)
                .map_err(|e| {
                    error!(workflow = %name, path = %path.display(), error = %e, "Failed to read workflow file");
                    EngineError::InvalidWorkflow(format!("Failed to read workflow {} from {}: {}", name, path.display(), e))
                })?;

            // Expand environment variables in the content
            let content = expand_env_vars(&content);

            let workflow = PreparedWorkflow::from_yaml(&content)
                .map_err(|e| {
                    error!(
                        workflow = %name,
                        path = %path.display(),
                        error = %e,
                        "Failed to parse workflow YAML"
                    );
                    EngineError::InvalidWorkflow(format!(
                        "Failed to parse workflow {} from {}: {}",
                        name, path.display(), e
                    ))
                })?;

            info!(workflow = %name, path = %path.display(), "Loaded workflow");
            workflows.insert(name, workflow);
        }
    }

    info!(count = workflows.len(), "Workflows loaded successfully");
    Ok(workflows)
}

/// Load all prompt template YAML files from a directory
async fn load_prompts(dir: &PathBuf) -> Result<HashMap<String, Template>, EngineError> {
    let mut templates = HashMap::new();

    debug!(directory = %dir.display(), "Loading prompt templates from directory");

    if !dir.exists() {
        error!(directory = %dir.display(), "Prompts directory does not exist");
        return Err(EngineError::InvalidWorkflow(
            format!("Prompts directory does not exist: {}", dir.display())
        ));
    }

    let entries = std::fs::read_dir(dir)
        .map_err(|e| {
            error!(directory = %dir.display(), error = %e, "Failed to read prompts directory");
            EngineError::InvalidWorkflow(format!("Failed to read prompts directory {}: {}", dir.display(), e))
        })?;

    for entry in entries {
        let entry = entry
            .map_err(|e| {
                error!(directory = %dir.display(), error = %e, "Failed to read directory entry");
                EngineError::InvalidWorkflow(format!("Failed to read directory entry: {}", e))
            })?;
        let path = entry.path();

        // Only process .yaml files
        if path.extension().map(|e| e == "yaml" || e == "yml").unwrap_or(false) {
            let template_id = path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    error!(path = %path.display(), "Invalid template file name");
                    EngineError::InvalidWorkflow(format!("Invalid template file name: {}", path.display()))
                })?
                .to_string();

            let content = std::fs::read_to_string(&path)
                .map_err(|e| {
                    error!(template = %template_id, path = %path.display(), error = %e, "Failed to read template file");
                    EngineError::InvalidWorkflow(format!("Failed to read template {} from {}: {}", template_id, path.display(), e))
                })?;

            let template: Template = serde_yaml::from_str(&content)
                .map_err(|e| {
                    error!(
                        template = %template_id,
                        path = %path.display(),
                        error = %e,
                        "Failed to parse template YAML"
                    );
                    EngineError::InvalidWorkflow(format!(
                        "Failed to parse template {} from {}: {}",
                        template_id, path.display(), e
                    ))
                })?;

            info!(template = %template_id, path = %path.display(), "Loaded template");
            templates.insert(template_id, template);
        }
    }

    info!(count = templates.len(), "Templates loaded successfully");
    Ok(templates)
}

/// Load all transform Rhai script files from a directory
fn load_transforms(dir: &PathBuf) -> Result<HashMap<String, String>, EngineError> {
    let mut transforms = HashMap::new();

    debug!(directory = %dir.display(), "Loading transform scripts from directory");

    if !dir.exists() {
        error!(directory = %dir.display(), "Transforms directory does not exist");
        return Err(EngineError::InvalidWorkflow(
            format!("Transforms directory does not exist: {}", dir.display())
        ));
    }

    let entries = std::fs::read_dir(dir)
        .map_err(|e| {
            error!(directory = %dir.display(), error = %e, "Failed to read transforms directory");
            EngineError::InvalidWorkflow(format!("Failed to read transforms directory {}: {}", dir.display(), e))
        })?;

    for entry in entries {
        let entry = entry
            .map_err(|e| {
                error!(directory = %dir.display(), error = %e, "Failed to read directory entry");
                EngineError::InvalidWorkflow(format!("Failed to read directory entry: {}", e))
            })?;
        let path = entry.path();

        // Only process .rhai files
        if path.extension().map(|e| e == "rhai").unwrap_or(false) {
            let script_name = path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    error!(path = %path.display(), "Invalid transform script file name");
                    EngineError::InvalidWorkflow(format!("Invalid transform script file name: {}", path.display()))
                })?
                .to_string();

            let content = std::fs::read_to_string(&path)
                .map_err(|e| {
                    error!(script = %script_name, path = %path.display(), error = %e, "Failed to read transform script");
                    EngineError::InvalidWorkflow(format!("Failed to read transform script {} from {}: {}", script_name, path.display(), e))
                })?;

            info!(script = %script_name, path = %path.display(), "Loaded transform script");
            transforms.insert(script_name, content);
        }
    }

    info!(count = transforms.len(), "Transform scripts loaded successfully");
    Ok(transforms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_builder_no_dirs() {
        let engine = EngineBuilder::new()
            .build()
            .await
            .unwrap();

        assert!(engine.list_workflows().is_empty());
    }

    #[tokio::test]
    async fn test_builder_with_provider() {
        let engine = EngineBuilder::new()
            .provider("test", "test-key")
            .build()
            .await
            .unwrap();

        assert!(engine.list_workflows().is_empty());
    }

    #[tokio::test]
    async fn test_builder_workflows_dir_not_found() {
        let result = EngineBuilder::new()
            .workflows_dir("/nonexistent/path")
            .build()
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_builder_load_workflows() {
        let dir = tempdir().unwrap();
        let workflow_path = dir.path().join("test.yaml");
        std::fs::write(&workflow_path, r#"
version: "0.1"
name: "Test"
nodes:
  - id: start
    type: start
    layer: 0
  - id: response
    type: response
    layer: 1
edges:
  - from: start
    to: response
"#).unwrap();

        let engine = EngineBuilder::new()
            .workflows_dir(dir.path())
            .build()
            .await
            .unwrap();

        assert!(engine.has_workflow("test"));
        assert_eq!(engine.list_workflows(), vec!["test"]);
    }
}
