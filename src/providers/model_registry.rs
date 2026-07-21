//! Model capability registry
//!
//! Tracks provider-level sampling rules and model-level capabilities
//! (json, thinking, vision, function calling, etc.)
//! Used to mask/remap ChatOptions before sending to provider APIs.

use std::collections::HashMap;
use serde::Deserialize;

/// Provider-level sampling configuration
#[derive(Debug, Clone)]
pub struct SamplingConfig {
    pub max_temperature: f32,
    pub supports_top_p: bool,
    pub supports_top_k: bool,
    pub default_top_p: Option<f32>,
    pub allow_both_temp_and_top_p: bool,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            max_temperature: 2.0,
            supports_top_p: true,
            supports_top_k: false,
            default_top_p: None,
            allow_both_temp_and_top_p: true,
        }
    }
}

/// Model-level capability flags
#[derive(Debug, Clone)]
pub struct ModelCapabilities {
    /// "none" | "optional" | "builtin"
    pub thinking: String,
    /// Supports JSON output mode (response_format)
    pub json: bool,
    /// Thinking + JSON compatibility: true / false / "partial"
    pub thinking_json_compat: ThinkingJsonCompat,
    /// Supports native function/tool calling
    pub function_calling: bool,
    /// Supports vision/image input
    pub vision: bool,
    /// Thinking budget configuration
    pub thinking_budget: Option<ThinkingBudgetConfig>,
    /// Per-model sampling overrides
    pub sampling_override: Option<SamplingOverride>,
    /// Max output tokens
    pub max_tokens: u32,
    /// Input price in USD per 1M tokens (None when unknown)
    pub input_price: Option<f64>,
    /// Output price in USD per 1M tokens (None when unknown)
    pub output_price: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ThinkingJsonCompat {
    Full,       // true — both work together
    Partial,    // "partial" — works with limitations
    None,       // false — incompatible
}

#[derive(Debug, Clone)]
pub struct ThinkingBudgetConfig {
    pub supports_thinking_budget: bool,
    pub budget_param_name: String,   // "reasoning_effort", "thinking_budget", "thinking_toggle"
    pub can_disable: bool,
    pub min_budget: Option<u32>,
    pub max_budget: Option<u32>,
    pub default_budget: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SamplingOverride {
    pub max_temperature: Option<f32>,
    pub allow_both_temp_and_top_p: Option<bool>,
}

/// Registry of model capabilities
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    /// Provider-level sampling config: provider_id -> SamplingConfig
    provider_sampling: HashMap<String, SamplingConfig>,
    /// Model-level capabilities: (provider_id, model_id) -> ModelCapabilities
    model_caps: HashMap<(String, String), ModelCapabilities>,
}

// ============================================================================
// JSON deserialization structs (match providers.json format)
// ============================================================================

#[derive(Deserialize)]
struct ProviderEntry {
    id: String,
    #[serde(default)]
    sampling: Option<SamplingJson>,
    models: Vec<ModelEntryJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SamplingJson {
    #[serde(default = "default_max_temp")]
    max_temperature: f32,
    #[serde(default = "default_true")]
    supports_top_p: bool,
    #[serde(default)]
    supports_top_k: bool,
    default_top_p: Option<f32>,
    #[serde(default = "default_true")]
    allow_both_temp_and_top_p: bool,
}

fn default_max_temp() -> f32 { 2.0 }
fn default_true() -> bool { true }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelEntryJson {
    id: String,
    #[serde(default)]
    max_tokens: u32,
    #[serde(default = "default_thinking")]
    thinking: String,
    #[serde(default)]
    json: bool,
    #[serde(default)]
    thinking_json_compat: serde_json::Value,
    #[serde(default)]
    function_calling: bool,
    #[serde(default)]
    vision: bool,
    #[serde(default)]
    thinking_budget: Option<ThinkingBudgetJson>,
    #[serde(default)]
    sampling_override: Option<SamplingOverrideJson>,
    #[serde(default)]
    input_price: Option<f64>,
    #[serde(default)]
    output_price: Option<f64>,
}

fn default_thinking() -> String { "none".to_string() }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingBudgetJson {
    #[serde(default)]
    supports_thinking_budget: bool,
    #[serde(default)]
    budget_param_name: String,
    #[serde(default)]
    can_disable: bool,
    min_budget: Option<u32>,
    max_budget: Option<u32>,
    default_budget: Option<u32>,
    min_max_tokens: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SamplingOverrideJson {
    max_temperature: Option<f32>,
    allow_both_temp_and_top_p: Option<bool>,
}

// ============================================================================
// Implementation
// ============================================================================

impl ModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from the embedded providers.json
    pub fn from_embedded() -> Self {
        let json = include_str!("providers.json");
        Self::from_json(json)
    }

    /// Parse providers.json and extract all capabilities
    pub fn from_json(json: &str) -> Self {
        let mut registry = Self::new();

        match serde_json::from_str::<Vec<ProviderEntry>>(json) {
            Ok(providers) => {
                for provider in &providers {
                    // Provider-level sampling
                    if let Some(ref sampling) = provider.sampling {
                        registry.provider_sampling.insert(
                            provider.id.clone(),
                            SamplingConfig {
                                max_temperature: sampling.max_temperature,
                                supports_top_p: sampling.supports_top_p,
                                supports_top_k: sampling.supports_top_k,
                                default_top_p: sampling.default_top_p,
                                allow_both_temp_and_top_p: sampling.allow_both_temp_and_top_p,
                            },
                        );
                    }

                    // Model-level capabilities
                    for model in &provider.models {
                        let thinking_json_compat = match &model.thinking_json_compat {
                            serde_json::Value::Bool(true) => ThinkingJsonCompat::Full,
                            serde_json::Value::String(s) if s == "partial" => ThinkingJsonCompat::Partial,
                            _ => ThinkingJsonCompat::None,
                        };

                        let thinking_budget = model.thinking_budget.as_ref().map(|tb| {
                            ThinkingBudgetConfig {
                                supports_thinking_budget: tb.supports_thinking_budget,
                                budget_param_name: tb.budget_param_name.clone(),
                                can_disable: tb.can_disable,
                                min_budget: tb.min_budget.or(tb.min_max_tokens),
                                max_budget: tb.max_budget,
                                default_budget: tb.default_budget,
                            }
                        });

                        let sampling_override = model.sampling_override.as_ref().map(|so| {
                            SamplingOverride {
                                max_temperature: so.max_temperature,
                                allow_both_temp_and_top_p: so.allow_both_temp_and_top_p,
                            }
                        });

                        registry.model_caps.insert(
                            (provider.id.clone(), model.id.clone()),
                            ModelCapabilities {
                                thinking: model.thinking.clone(),
                                json: model.json,
                                thinking_json_compat,
                                function_calling: model.function_calling,
                                vision: model.vision,
                                thinking_budget,
                                sampling_override,
                                max_tokens: model.max_tokens,
                                input_price: model.input_price,
                                output_price: model.output_price,
                            },
                        );
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to parse providers.json for model registry");
            }
        }

        registry
    }

    /// Get provider sampling config
    pub fn get_sampling(&self, provider: &str) -> Option<&SamplingConfig> {
        self.provider_sampling.get(provider)
    }

    /// Get model capabilities
    pub fn get_capabilities(&self, provider: &str, model: &str) -> Option<&ModelCapabilities> {
        self.model_caps.get(&(provider.to_string(), model.to_string()))
    }

    /// Check if a model supports vision
    pub fn supports_vision(&self, provider: &str, model: &str) -> bool {
        self.get_capabilities(provider, model)
            .map(|c| c.vision)
            .unwrap_or(false)
    }

    /// Check if a model supports JSON output mode
    pub fn supports_json(&self, provider: &str, model: &str) -> bool {
        self.get_capabilities(provider, model)
            .map(|c| c.json)
            .unwrap_or(false)
    }

    /// Check if a model supports function calling
    pub fn supports_function_calling(&self, provider: &str, model: &str) -> bool {
        self.get_capabilities(provider, model)
            .map(|c| c.function_calling)
            .unwrap_or(false)
    }

    /// Check if a model supports thinking
    pub fn supports_thinking(&self, provider: &str, model: &str) -> bool {
        self.get_capabilities(provider, model)
            .map(|c| c.thinking != "none")
            .unwrap_or(false)
    }

    /// Get the effective max temperature for a model
    pub fn max_temperature(&self, provider: &str, model: &str) -> f32 {
        // Check model-level override first, then provider-level
        if let Some(caps) = self.get_capabilities(provider, model) {
            if let Some(ref over) = caps.sampling_override {
                if let Some(mt) = over.max_temperature {
                    return mt;
                }
            }
        }
        self.get_sampling(provider)
            .map(|s| s.max_temperature)
            .unwrap_or(2.0)
    }

    /// Register a single model's capabilities manually
    pub fn register_vision_model(&mut self, provider: &str, model: &str) {
        self.model_caps
            .entry((provider.to_string(), model.to_string()))
            .and_modify(|c| c.vision = true)
            .or_insert(ModelCapabilities {
                thinking: "none".to_string(),
                json: true,
                thinking_json_compat: ThinkingJsonCompat::Full,
                function_calling: true,
                vision: true,
                thinking_budget: None,
                sampling_override: None,
                max_tokens: 4096,
                input_price: None,
                output_price: None,
            });
    }

    /// Look up (input_price, output_price) in USD per 1M tokens.
    /// Returns None if the model is unknown or pricing is missing.
    pub fn get_pricing(&self, provider: &str, model: &str) -> Option<(f64, f64)> {
        let caps = self.get_capabilities(provider, model)?;
        match (caps.input_price, caps.output_price) {
            (Some(i), Some(o)) => Some((i, o)),
            _ => None,
        }
    }

    /// Compute cost in USD for given input/output token counts.
    /// Returns None if pricing is unknown.
    pub fn compute_cost(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Option<f64> {
        let (input_price, output_price) = self.get_pricing(provider, model)?;
        let cost = (input_tokens as f64) * input_price / 1_000_000.0
            + (output_tokens as f64) * output_price / 1_000_000.0;
        Some(cost)
    }

    /// Get all vision-capable models (for backward compat)
    pub fn vision_models(&self) -> std::collections::HashSet<(String, String)> {
        self.model_caps.iter()
            .filter(|(_, caps)| caps.vision)
            .map(|((p, m), _)| (p.clone(), m.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_embedded() {
        let registry = ModelRegistry::from_embedded();

        // Vision checks
        assert!(registry.supports_vision("openai", "gpt-4o"));
        assert!(registry.supports_vision("anthropic", "claude-sonnet-4-5-20250929"));
        assert!(!registry.supports_vision("qwen", "qwen-flash"));

        // JSON support
        assert!(registry.supports_json("openai", "gpt-4o"));
        assert!(registry.supports_json("anthropic", "claude-sonnet-4-5-20250929"));
        assert!(registry.supports_json("atlascloud", "qwen/qwen3.5-flash"));

        // Function calling
        assert!(registry.supports_function_calling("openai", "gpt-4o"));
        assert!(!registry.supports_function_calling("minimax", "M2-her"));
        assert!(registry.supports_function_calling("atlascloud", "deepseek-ai/deepseek-v4-pro"));

        // Thinking
        assert!(registry.supports_thinking("openai", "gpt-5"));
        assert!(!registry.supports_thinking("openai", "gpt-4o"));

        // Sampling
        let sampling = registry.get_sampling("openai").unwrap();
        assert!(sampling.supports_top_p);
        assert!(!sampling.supports_top_k);

        // Model caps
        let caps = registry.get_capabilities("openai", "o3").unwrap();
        assert_eq!(caps.thinking, "builtin");
        assert!(caps.thinking_budget.is_some());
        assert!(caps.sampling_override.is_some());

        // Max temperature with override
        assert_eq!(registry.max_temperature("openai", "gpt-4o"), 2.0);
        assert_eq!(registry.max_temperature("openai", "o3"), 1.0); // samplingOverride
    }

    #[test]
    fn test_pricing_and_cost() {
        let registry = ModelRegistry::from_embedded();

        // Known model: gpt-5.4 at $2.5/$15
        let (i, o) = registry.get_pricing("openai", "gpt-5.4").unwrap();
        assert!((i - 2.5).abs() < 1e-9);
        assert!((o - 15.0).abs() < 1e-9);

        // Cost: 1M input + 1M output = 2.5 + 15 = 17.5
        let cost = registry.compute_cost("openai", "gpt-5.4", 1_000_000, 1_000_000).unwrap();
        assert!((cost - 17.5).abs() < 1e-9);

        // 1k tokens each: 0.0025 + 0.015 = 0.0175
        let small = registry.compute_cost("openai", "gpt-5.4", 1_000, 1_000).unwrap();
        assert!((small - 0.0175).abs() < 1e-9);

        // Unknown model: None
        assert!(registry.get_pricing("fake", "model").is_none());
        assert!(registry.compute_cost("fake", "model", 1000, 1000).is_none());
    }

    #[test]
    fn test_unknown_model_defaults() {
        let registry = ModelRegistry::from_embedded();
        // Unknown model returns None/false
        assert!(!registry.supports_vision("fake", "model"));
        assert!(!registry.supports_json("fake", "model"));
        assert!(registry.get_capabilities("fake", "model").is_none());
    }
}
