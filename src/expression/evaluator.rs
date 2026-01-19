//! Expression evaluator

use serde_json::Value;

use crate::error::ExprError;
use crate::state::ExecutionState;
use super::lexer::Lexer;
use super::parser::{Expr, PathExpr, CompareOp, PathSegment, parse};

/// Evaluate an expression string against the execution state
pub fn evaluate_expression(expr: &str, state: &ExecutionState) -> Result<bool, ExprError> {
    let mut lexer = Lexer::new(expr);
    let tokens = lexer.tokenize()?;
    let ast = parse(tokens)?;
    eval_ast(&ast, state)
}

/// Evaluate the AST to a boolean
fn eval_ast(expr: &Expr, state: &ExecutionState) -> Result<bool, ExprError> {
    match expr {
        Expr::Or(left, right) => {
            Ok(eval_ast(left, state)? || eval_ast(right, state)?)
        }
        Expr::And(left, right) => {
            Ok(eval_ast(left, state)? && eval_ast(right, state)?)
        }
        Expr::Not(inner) => {
            Ok(!eval_ast(inner, state)?)
        }
        Expr::Compare(left, op, right) => {
            let left_val = eval_to_value(left, state)?;
            let right_val = eval_to_value(right, state)?;
            Ok(compare_values(&left_val, op, &right_val))
        }
        Expr::Path(path) => {
            // Bare path is truthy check
            let value = resolve_path(path, state)?;
            Ok(is_truthy(&value))
        }
        Expr::Literal(val) => {
            Ok(is_truthy(val))
        }
        Expr::FunctionCall(name, arg) => {
            let arg_val = eval_to_value(arg, state)?;
            match name.as_str() {
                "empty" => Ok(is_empty(&arg_val)),
                "exists" => Ok(!arg_val.is_null()),
                "len" => Ok(get_length(&arg_val) > 0),
                _ => Err(ExprError::UnknownFunction(name.clone())),
            }
        }
    }
}

/// Evaluate an expression to a Value
fn eval_to_value(expr: &Expr, state: &ExecutionState) -> Result<Value, ExprError> {
    match expr {
        Expr::Path(path) => resolve_path(path, state),
        Expr::Literal(val) => Ok(val.clone()),
        Expr::FunctionCall(name, arg) => {
            let arg_val = eval_to_value(arg, state)?;
            match name.as_str() {
                "empty" => Ok(Value::Bool(is_empty(&arg_val))),
                "exists" => Ok(Value::Bool(!arg_val.is_null())),
                "len" => Ok(Value::Number(get_length(&arg_val).into())),
                _ => Err(ExprError::UnknownFunction(name.clone())),
            }
        }
        _ => {
            // For compound expressions, evaluate as boolean
            Ok(Value::Bool(eval_ast(expr, state)?))
        }
    }
}

/// Resolve a path expression to a Value
fn resolve_path(path: &PathExpr, state: &ExecutionState) -> Result<Value, ExprError> {
    let path_str = path_to_string(path);
    state.resolve_path(&path_str)
}

/// Convert PathExpr to string for state resolution
fn path_to_string(path: &PathExpr) -> String {
    let mut result = String::new();

    for (i, segment) in path.segments.iter().enumerate() {
        match segment {
            PathSegment::Field(name) => {
                if i > 0 {
                    result.push('.');
                }
                result.push_str(name);
            }
            PathSegment::Index(idx) => {
                result.push('[');
                result.push_str(&idx.to_string());
                result.push(']');
            }
            PathSegment::ShortId(sid) => {
                result.push('[');
                result.push_str(sid);
                result.push(']');
            }
            PathSegment::All => {
                result.push_str("[*]");
            }
        }
    }

    result
}

/// Compare two values
fn compare_values(left: &Value, op: &CompareOp, right: &Value) -> bool {
    match op {
        CompareOp::Eq => values_equal(left, right),
        CompareOp::Ne => !values_equal(left, right),
        CompareOp::Gt => compare_numeric(left, right) == Some(std::cmp::Ordering::Greater),
        CompareOp::Lt => compare_numeric(left, right) == Some(std::cmp::Ordering::Less),
        CompareOp::Ge => matches!(compare_numeric(left, right), Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)),
        CompareOp::Le => matches!(compare_numeric(left, right), Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)),
    }
}

/// Check if two values are equal
fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => {
            a.as_f64() == b.as_f64()
        }
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => a == b,
        (Value::Object(a), Value::Object(b)) => a == b,
        // Cross-type comparison
        (Value::String(s), Value::Bool(b)) | (Value::Bool(b), Value::String(s)) => {
            (s == "true" && *b) || (s == "false" && !*b)
        }
        _ => false,
    }
}

/// Compare numeric values
fn compare_numeric(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    let left_num = match left {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }?;

    let right_num = match right {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }?;

    left_num.partial_cmp(&right_num)
}

/// Check if a value is truthy
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Check if a value is empty
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(_) => false,
        Value::Number(_) => false,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// Get the length of a value
fn get_length(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::Array(a) => a.len(),
        Value::Object(o) => o.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_simple_comparison() {
        let mut state = ExecutionState::new(HashMap::new());

        let mut outputs = HashMap::new();
        outputs.insert("done".to_string(), Value::Bool(true));
        state.write("think", outputs);

        assert!(evaluate_expression("think.done == true", &state).unwrap());
        assert!(!evaluate_expression("think.done == false", &state).unwrap());
    }

    #[test]
    fn test_not_expression() {
        let mut state = ExecutionState::new(HashMap::new());

        let mut outputs = HashMap::new();
        outputs.insert("done".to_string(), Value::Bool(false));
        state.write("think", outputs);

        assert!(evaluate_expression("!think.done", &state).unwrap());
    }

    #[test]
    fn test_empty_function() {
        let mut state = ExecutionState::new(HashMap::new());

        let mut outputs = HashMap::new();
        outputs.insert("items".to_string(), Value::Array(vec![]));
        state.write("node", outputs);

        assert!(evaluate_expression("empty(node.items)", &state).unwrap());
    }

    #[test]
    fn test_and_or() {
        let mut state = ExecutionState::new(HashMap::new());

        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), Value::Bool(true));
        outputs.insert("b".to_string(), Value::Bool(false));
        state.write("node", outputs);

        assert!(!evaluate_expression("node.a && node.b", &state).unwrap());
        assert!(evaluate_expression("node.a || node.b", &state).unwrap());
    }

    #[test]
    fn test_numeric_comparison() {
        let mut state = ExecutionState::new(HashMap::new());

        let mut outputs = HashMap::new();
        outputs.insert("count".to_string(), Value::Number(5.into()));
        state.write("node", outputs);

        assert!(evaluate_expression("node.count > 3", &state).unwrap());
        assert!(!evaluate_expression("node.count > 10", &state).unwrap());
    }

    #[test]
    fn test_input_path() {
        let mut input = HashMap::new();
        input.insert("query".to_string(), Value::String("hello".to_string()));

        let state = ExecutionState::new(input);

        assert!(evaluate_expression("input.query == \"hello\"", &state).unwrap());
    }
}
