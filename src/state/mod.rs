//! State management with append-only semantics and short ID references

mod execution_state;

pub use execution_state::{ExecutionState, ExecutionRecord, SupervisorContext, CheckpointState, EdgeCountEntry};
// Internal: executor needs wall-clock to stamp per-execution start times.
pub(crate) use execution_state::current_timestamp;
