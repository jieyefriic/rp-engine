//! Prompt template definitions

use serde::{Deserialize, Serialize};

/// A prompt template attached to the workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    /// User prompt with {{variable}} placeholders
    pub user_prompt: String,

    /// Optional system prompt
    pub system_prompt: Option<String>,

    /// Model configuration
    #[serde(default)]
    pub config: TemplateConfig,

    /// Optional output schema for structured responses
    #[serde(default)]
    pub output_schema: Vec<OutputField>,
}

/// Configuration for LLM calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateConfig {
    /// Temperature for sampling (0.0 - 2.0)
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// Maximum tokens to generate
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,

    /// Top-p sampling
    pub top_p: Option<f32>,

    /// Top-k sampling
    pub top_k: Option<u32>,

    /// Thinking budget for extended thinking models
    pub thinking_budget: Option<u32>,

    /// Enable web search for supported providers (e.g., DashScope/Qwen)
    #[serde(default)]
    pub enable_search: bool,
}

impl Default for TemplateConfig {
    fn default() -> Self {
        Self {
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            top_p: None,
            top_k: None,
            thinking_budget: None,
            enable_search: false,
        }
    }
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> u32 {
    2048
}

/// A field in the output schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputField {
    /// Field name in the output JSON
    pub name: String,

    /// Field type: string, number, boolean, string[], number[], object, object[]
    #[serde(rename = "type")]
    pub field_type: String,

    /// Optional description for the field
    pub description: Option<String>,

    /// Optional example value
    pub example: Option<String>,
}

impl Template {
    /// Create a simple template with just a user prompt
    pub fn simple(user_prompt: impl Into<String>) -> Self {
        Self {
            user_prompt: user_prompt.into(),
            system_prompt: None,
            config: TemplateConfig::default(),
            output_schema: Vec::new(),
        }
    }

    /// Add a system prompt
    pub fn with_system(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    /// Add an output field to the schema
    pub fn with_output_field(mut self, name: impl Into<String>, field_type: impl Into<String>) -> Self {
        self.output_schema.push(OutputField {
            name: name.into(),
            field_type: field_type.into(),
            description: None,
            example: None,
        });
        self
    }

    /// Check if this template has a structured output schema
    pub fn has_output_schema(&self) -> bool {
        !self.output_schema.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_config_enable_search() {
        let yaml = r#"
temperature: 0.8
max_tokens: 500
enable_search: true
"#;
        let config: TemplateConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.enable_search);
        assert_eq!(config.temperature, 0.8);
        assert_eq!(config.max_tokens, 500);
    }

    #[test]
    fn test_template_config_enable_search_default() {
        let yaml = r#"
temperature: 0.7
max_tokens: 1024
"#;
        let config: TemplateConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.enable_search);
    }
}
