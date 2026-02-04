//! State management with append-only semantics and short ID references

mod execution_state;

pub use execution_state::{ExecutionState, ExecutionRecord, SupervisorContext, CheckpointState, EdgeCountEntry};
