//! YAML schema definitions for workflow structures

mod workflow;
mod node;
mod edge;
mod template;

pub use workflow::{PreparedWorkflow, Subgraph, ToolRef, InputSchema, InputField, DataSourceDef};
pub use node::{YamlNode, NodeConfig, NodeType};
pub use edge::{YamlEdge, ExitReason};
pub use template::{Template, TemplateConfig, OutputField};
