//! Stream handler trait for real-time text chunk delivery
//!
//! The `StreamHandler` trait provides a transport-agnostic interface for
//! delivering text chunks from generate nodes during streaming LLM calls.
//!
//! The engine itself does not define how chunks are delivered to end users.
//! The business layer implements this trait with a concrete transport
//! (e.g., Redis pub/sub, SSE, WebSocket).

use std::sync::Arc;

/// Handler for streaming text chunks from generate nodes.
///
/// Implementations should be lightweight and non-blocking.
/// For async operations (e.g., Redis publish), use `tokio::spawn` internally.
///
/// # Arguments to `on_text_chunk`
/// - `node_id`: ID of the generate node producing the chunk
/// - `chunk`: The new text fragment
/// - `accumulated`: The full text accumulated so far
pub trait StreamHandler: Send + Sync {
    fn on_text_chunk(&self, node_id: &str, chunk: &str, accumulated: &str);
}

/// Type alias for shared stream handler reference
pub type SharedStreamHandler = Arc<dyn StreamHandler>;
