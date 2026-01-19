//! Expression language for edge conditions
//!
//! Supports:
//! - Comparison: ==, !=, >, <, >=, <=
//! - Logical: &&, ||, !
//! - Functions: empty(), exists(), len()
//! - Path expressions with short_id selectors

mod lexer;
mod parser;
mod evaluator;

pub use evaluator::evaluate_expression;
pub use parser::{Expr, PathExpr, CompareOp};
