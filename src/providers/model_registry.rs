//! Model capability registry
//!
//! Tracks which provider/model combinations support specific capabilities
//! (e.g., vision). Used for compile-time validation of workflow graphs.

use std::collections::HashSet;
use serde::Deserialize;

/// Registry of model capabilities
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    /// Models that support vision: (provider_id, model_id)
    vision_models: HashSet<(String, String)>,
}

/// JSON structure for parsing providers.json
#[derive(Deserialize)]
struct ProviderEntry {
    id: String,
    models: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    vision: bool,
}

impl ModelRegistry {
    /// Create an empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from the embedded providers.json (default model catalog)
    pub fn from_embedded() -> Self {
        let json = include_str!("providers.json");
        Self::from_json(json)
    }

    /// Parse a providers.json-format string and extract vision capabilities
    pub fn from_json(json: &str) -> Self {
        let mut registry = Self::new();

        match serde_json::from_str::<Vec<ProviderEntry>>(json) {
            Ok(providers) => {
                for provider in &providers {
                    for model in &provider.models {
                        if model.vision {
                            registry.vision_models.insert((
                                provider.id.clone(),
                                model.id.clone(),
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to parse providers.json for model registry");
            }
        }

        registry
    }

    /// Register a single model as vision-capable
    pub fn register_vision_model(&mut self, provider: &str, model: &str) {
        self.vision_models.insert((provider.to_string(), model.to_string()));
    }

    /// Check if a model supports vision
    pub fn supports_vision(&self, provider: &str, model: &str) -> bool {
        self.vision_models.contains(&(provider.to_string(), model.to_string()))
    }

    /// Get all vision-capable models
    pub fn vision_models(&self) -> &HashSet<(String, String)> {
        &self.vision_models
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_embedded() {
        let registry = ModelRegistry::from_embedded();
        // OpenAI GPT-4o should support vision
        assert!(registry.supports_vision("openai", "gpt-4o"));
        assert!(registry.supports_vision("openai", "gpt-4o-mini"));
        // Anthropic Claude should support vision
        assert!(registry.supports_vision("anthropic", "claude-sonnet-4-5-20250929"));
        // Gemini should support vision
        assert!(registry.supports_vision("gemini", "gemini-2.0-flash"));
        // Qwen VL models should support vision
        assert!(registry.supports_vision("qwen", "qwen-vl-plus"));
        assert!(registry.supports_vision("qwen", "qwen-vl-max"));
        // Non-vision models should NOT
        assert!(!registry.supports_vision("deepseek", "deepseek-chat"));
        assert!(!registry.supports_vision("qwen", "qwen-turbo"));
        assert!(!registry.supports_vision("qwen", "qwen-flash"));
    }

    #[test]
    fn test_from_json() {
        let json = r#"[
            {
                "id": "test-provider",
                "name": "Test",
                "defaultModel": "model-a",
                "sampling": { "maxTemperature": 2, "supportsTopP": true, "supportsTopK": false, "allowBothTempAndTopP": true },
                "models": [
                    { "id": "model-a", "name": "Model A", "modelType": "chat", "contextWindow": 4096, "maxTokens": 1024, "inputPrice": 1, "outputPrice": 2, "thinking": "none", "json": true, "thinkingJsonCompat": true, "functionCalling": true, "vision": true },
                    { "id": "model-b", "name": "Model B", "modelType": "chat", "contextWindow": 4096, "maxTokens": 1024, "inputPrice": 1, "outputPrice": 2, "thinking": "none", "json": true, "thinkingJsonCompat": true, "functionCalling": true, "vision": false }
                ]
            }
        ]"#;

        let registry = ModelRegistry::from_json(json);
        assert!(registry.supports_vision("test-provider", "model-a"));
        assert!(!registry.supports_vision("test-provider", "model-b"));
    }

    #[test]
    fn test_register_vision_model() {
        let mut registry = ModelRegistry::new();
        assert!(!registry.supports_vision("custom", "my-vl-model"));

        registry.register_vision_model("custom", "my-vl-model");
        assert!(registry.supports_vision("custom", "my-vl-model"));
    }

    #[test]
    fn test_unknown_model() {
        let registry = ModelRegistry::from_embedded();
        assert!(!registry.supports_vision("nonexistent", "fake-model"));
    }
}
