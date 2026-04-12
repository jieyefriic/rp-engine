//! YAML schema definitions for workflow structures

mod workflow;
mod node;
mod edge;
mod template;

pub use workflow::{PreparedWorkflow, Subgraph, ToolRef, InputSchema, InputField, DataSourceDef, ProviderDef, HarnessConfig, WritebackConfig};
pub use node::{YamlNode, NodeConfig, NodeType, VariableBinding, TemplateSelector, TemplateCondition, SkillDef};
pub use edge::{YamlEdge, ExitReason};
pub use template::{Template, TemplateConfig, OutputField};
