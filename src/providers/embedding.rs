//! Embedding Provider trait and registry
//!
//! Supports multiple embedding providers: DashScope, OpenAI
//! All providers implement the EmbeddingProvider trait with embed() method.

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

// ============================================================================
// Embedding Options
// ============================================================================

/// Embedding request options
#[derive(Debug, Clone, Default)]
pub struct EmbeddingOptions {
    /// Model to use (provider default if None)
    pub model: Option<String>,
    /// Output dimension (for models that support variable dimensions)
    pub dimension: Option<usize>,
}

impl EmbeddingOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_dimension(mut self, dimension: usize) -> Self {
        self.dimension = Some(dimension);
        self
    }
}

// ============================================================================
// Response Types
// ============================================================================

/// Token usage information for embeddings
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbeddingUsage {
    pub prompt_tokens: u64,
    pub total_tokens: u64,
}

/// Single embedding result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    /// The embedding vector
    pub vector: Vec<f32>,
    /// Index in the input array
    pub index: usize,
}

/// Embedding response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    /// List of embeddings (one per input text)
    pub embeddings: Vec<Embedding>,
    /// Token usage information
    pub usage: EmbeddingUsage,
    /// Model used
    pub model: String,
}

// ============================================================================
// Provider Trait
// ============================================================================

/// Embedding Provider trait - all embedding providers implement this
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Get the provider name (e.g., "dashscope", "openai")
    fn name(&self) -> &str;

    /// Get the default model for this provider
    fn default_model(&self) -> &str;

    /// Create embeddings for a list of texts
    async fn embed(
        &self,
        texts: Vec<String>,
        options: EmbeddingOptions,
    ) -> Result<EmbeddingResponse, ProviderError>;
}

// ============================================================================
// Provider Registry
// ============================================================================

/// Embedding provider registry for managing multiple providers
#[derive(Clone, Default)]
pub struct EmbeddingRegistry {
    providers: HashMap<String, Arc<dyn EmbeddingProvider>>,
}

impl EmbeddingRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self { providers: HashMap::new() }
    }

    /// Register a provider
    pub fn register(&mut self, name: impl Into<String>, provider: Arc<dyn EmbeddingProvider>) {
        self.providers.insert(name.into(), provider);
    }

    /// Get a provider by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn EmbeddingProvider>> {
        self.providers.get(name).cloned()
    }

    /// Check if a provider exists
    pub fn has(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get all registered provider names
    pub fn names(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}
