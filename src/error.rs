//! Unified error types for RicePrompt-Engine

use thiserror::Error;

/// Main error type for the engine
#[derive(Error, Debug)]
pub enum EngineError {
    // Workflow errors
    #[error("Invalid workflow: {0}")]
    InvalidWorkflow(String),

    #[error("No start node found in workflow")]
    NoStartNode,

    #[error("Multiple start nodes found")]
    MultipleStartNodes,

    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Circular dependency detected")]
    CircularDependency,

    // Execution errors
    #[error("Max iterations exceeded for edge {from} -> {to}")]
    MaxIterationsExceeded { from: String, to: String },

    #[error("No matching route from node: {0}")]
    NoMatchingRoute(String),

    #[error("Node execution failed: {0}")]
    NodeExecutionFailed(String),

    // Template errors
    #[error("Template not found: {0}")]
    TemplateNotFound(String),

    #[error("Missing template reference")]
    MissingTemplate,

    // Provider errors
    #[error("Provider not found: {0}")]
    ProviderNotFound(String),

    #[error("Provider error: {0}")]
    ProviderError(#[from] ProviderError),

    // State errors
    #[error("Path not found: {0}")]
    PathNotFound(String),

    #[error("Short ID not found: {0}")]
    ShortIdNotFound(String),

    #[error("No executions for node: {0}")]
    NoExecutions(String),

    // Expression errors
    #[error("Expression error: {0}")]
    ExpressionError(#[from] ExprError),

    // Supervisor errors
    #[error("Supervisor tool not found: {0}")]
    SupervisorToolNotFound(String),

    #[error("Supervisor subgraph not found: {0}")]
    SupervisorSubgraphNotFound(String),

    #[error("Sub-workflow not found: {0}")]
    WorkflowNotFound(String),

    #[error("Supervisor planner failed to generate valid todos")]
    SupervisorPlannerFailed,

    #[error("Supervisor max iterations exceeded")]
    SupervisorMaxIterations,

    #[error("Supervisor timeout")]
    SupervisorTimeout,

    #[error("Supervisor tool failures exceeded: {count} failures")]
    SupervisorToolFailures { count: u32 },

    // Data connector errors
    #[error("Data source not found: {0}")]
    DataSourceNotFound(String),

    #[error("Missing source configuration")]
    MissingSource,

    #[error("Unsupported source type: {0}")]
    UnsupportedSource(String),

    #[error("Missing connection configuration")]
    MissingConnection,

    #[error("Missing URL in connection")]
    MissingUrl,

    #[error("Invalid HTTP method: {0}")]
    InvalidMethod(String),

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("MinIO error: {0}")]
    MinIOError(String),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Qdrant error: {0}")]
    QdrantError(String),

    #[error("Redis error: {0}")]
    RedisError(String),

    // State saving errors
    #[error("State error: {0}")]
    StateError(String),

    // Serialization errors
    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Provider-specific errors
#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("API error: {status} - {message}")]
    ApiError { status: u16, message: String },

    #[error("Rate limited")]
    RateLimited,

    #[error("Authentication failed")]
    AuthenticationFailed,

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Request failed: {0}")]
    RequestFailed(String),

    #[error("Timeout")]
    Timeout,
}

/// Expression parsing/evaluation errors
#[derive(Error, Debug)]
pub enum ExprError {
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),

    #[error("Unexpected end of expression")]
    UnexpectedEnd,

    #[error("Unknown function: {0}")]
    UnknownFunction(String),

    #[error("Type mismatch: expected {expected}, got {got}")]
    TypeMismatch { expected: String, got: String },

    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Short ID not found: {0}")]
    ShortIdNotFound(String),

    #[error("No executions for node")]
    NoExecutions,

    #[error("Index out of bounds: {0}")]
    IndexOutOfBounds(i32),

    #[error("Parse error: {0}")]
    ParseError(String),
}

/// Validation errors for workflow YAML
#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid node type: {0}")]
    InvalidNodeType(String),

    #[error("Invalid edge: {from} -> {to}")]
    InvalidEdge { from: String, to: String },

    #[error("Duplicate node ID: {0}")]
    DuplicateNodeId(String),

    #[error("Invalid template reference: {0}")]
    InvalidTemplateRef(String),

    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),
}
