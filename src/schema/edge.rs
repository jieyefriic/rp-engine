//! Edge definitions for workflow graphs

use serde::{Deserialize, Serialize};

/// Supervisor exit reason types
/// Used for exit edge routing from Supervisor nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    /// All tasks completed (todo list is empty)
    TodoEmpty,
    /// Execution timeout exceeded
    Timeout,
    /// Maximum iterations reached
    MaxIterations,
    /// Result length limit exceeded
    MaxResultLength,
    /// Too many tool failures
    ToolFailures,
    /// Default - matches any exit reason
    Default,
}

impl Default for ExitReason {
    fn default() -> Self {
        ExitReason::Default
    }
}

/// An edge connecting two nodes in the workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YamlEdge {
    /// Source node ID
    pub from: String,

    /// Target node ID
    pub to: String,

    /// Optional condition expression
    /// If present, edge is only taken when condition evaluates to true
    pub condition: Option<String>,

    /// Maximum number of times this edge can be traversed
    /// Used for loop protection
    pub max_iterations: Option<u32>,

    /// Whether this is a default edge (taken when no conditional edges match)
    #[serde(default)]
    pub is_default: bool,

    /// Exit reason for edges from Supervisor nodes
    /// When set, this edge is taken when Supervisor exits with matching reason
    #[serde(default)]
    pub exit_reason: Option<ExitReason>,
}

impl YamlEdge {
    /// Create a new unconditional edge
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            condition: None,
            max_iterations: None,
            is_default: false,
            exit_reason: None,
        }
    }

    /// Add a condition to this edge
    pub fn with_condition(mut self, condition: impl Into<String>) -> Self {
        self.condition = Some(condition.into());
        self
    }

    /// Set max iterations for loop protection
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Mark this as a default edge
    pub fn as_default(mut self) -> Self {
        self.is_default = true;
        self
    }

    /// Set exit reason for Supervisor exit edges
    pub fn with_exit_reason(mut self, reason: ExitReason) -> Self {
        self.exit_reason = Some(reason);
        self
    }

    /// Check if this is an exit edge (has exit_reason set)
    pub fn is_exit_edge(&self) -> bool {
        self.exit_reason.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_reason_serialization() {
        let reason = ExitReason::TodoEmpty;
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, "\"todo_empty\"");

        let parsed: ExitReason = serde_json::from_str("\"max_iterations\"").unwrap();
        assert_eq!(parsed, ExitReason::MaxIterations);
    }

    #[test]
    fn test_edge_with_exit_reason() {
        let edge = YamlEdge::new("supervisor", "handler")
            .with_exit_reason(ExitReason::TodoEmpty);

        assert!(edge.is_exit_edge());
        assert_eq!(edge.exit_reason, Some(ExitReason::TodoEmpty));
    }

    #[test]
    fn test_edge_yaml_deserialization() {
        let yaml = r#"
from: node_2
to: node_3
exit_reason: todo_empty
"#;
        let edge: YamlEdge = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(edge.exit_reason, Some(ExitReason::TodoEmpty));
    }
}
