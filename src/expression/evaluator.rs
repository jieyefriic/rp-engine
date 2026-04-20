//! Expression evaluator

use regex_lite::Regex;
use serde_json::Value;
use tracing::{trace, warn};

use crate::error::ExprError;
use crate::state::ExecutionState;
use super::lexer::Lexer;
use super::parser::{Expr, PathExpr, CompareOp, PathSegment, parse};

/// Validate that a function was called with the expected number of args.
/// Returns an UnexpectedToken error mentioning the function name and
/// expected/actual arity — mirrors the existing error style in this module.
fn check_arity(name: &str, args: &[Expr], expected: usize) -> Result<(), ExprError> {
    if args.len() != expected {
        return Err(ExprError::UnexpectedToken(format!(
            "{name}() takes {expected} argument(s), got {}",
            args.len()
        )));
    }
    Ok(())
}

/// `contains(haystack, needle)`:
/// - Both strings → substring match
/// - haystack array + needle scalar → array contains element (JSON equality)
/// - otherwise → false (non-error: composes well in OR chains)
fn value_contains(haystack: &Value, needle: &Value) -> bool {
    match (haystack, needle) {
        (Value::String(h), Value::String(n)) => h.contains(n.as_str()),
        (Value::Array(arr), n) => arr.iter().any(|item| item == n),
        _ => false,
    }
}

/// `matches(text, pattern)`:
/// - Both strings → regex test using `regex_lite` (no fancy features; fine
///   for edge conditions)
/// - otherwise → TypeMismatch
///
/// The pattern is compiled on every call. Workflow edge conditions are
/// evaluated at most once per node ready-check, so caching isn't worth the
/// complexity right now.
fn value_matches(text: &Value, pattern: &Value) -> Result<bool, ExprError> {
    let (t, p) = match (text, pattern) {
        (Value::String(t), Value::String(p)) => (t, p),
        _ => {
            return Err(ExprError::TypeMismatch {
                expected: "string, string".into(),
                got: format!("{:?}, {:?}", text, pattern),
            })
        }
    };
    match Regex::new(p) {
        Ok(re) => Ok(re.is_match(t)),
        Err(e) => Err(ExprError::UnexpectedToken(format!(
            "invalid regex in matches(): {e}"
        ))),
    }
}

/// Evaluate an expression string against the execution state
pub fn evaluate_expression(expr: &str, state: &ExecutionState) -> Result<bool, ExprError> {
    trace!(expression = %expr, "Evaluating expression");

    let mut lexer = Lexer::new(expr);
    let tokens = lexer.tokenize().map_err(|e| {
        warn!(
            expression = %expr,
            error = %e,
            "Expression tokenization failed"
        );
        e
    })?;

    let ast = parse(tokens).map_err(|e| {
        warn!(
            expression = %expr,
            error = %e,
            "Expression parsing failed"
        );
        e
    })?;

    let result = eval_ast(&ast, state).map_err(|e| {
        warn!(
            expression = %expr,
            error = %e,
            "Expression evaluation failed"
        );
        e
    })?;

    trace!(expression = %expr, result = result, "Expression evaluated");
    Ok(result)
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
        Expr::FunctionCall(name, args) => {
            match name.as_str() {
                "empty" => {
                    check_arity("empty", args, 1)?;
                    let v = eval_to_value(&args[0], state)?;
                    Ok(is_empty(&v))
                }
                "exists" => {
                    check_arity("exists", args, 1)?;
                    let v = eval_to_value(&args[0], state)?;
                    Ok(!v.is_null())
                }
                "len" => {
                    check_arity("len", args, 1)?;
                    let v = eval_to_value(&args[0], state)?;
                    Ok(get_length(&v) > 0)
                }
                "contains" => {
                    check_arity("contains", args, 2)?;
                    let h = eval_to_value(&args[0], state)?;
                    let n = eval_to_value(&args[1], state)?;
                    Ok(value_contains(&h, &n))
                }
                "not_contains" => {
                    check_arity("not_contains", args, 2)?;
                    let h = eval_to_value(&args[0], state)?;
                    let n = eval_to_value(&args[1], state)?;
                    Ok(!value_contains(&h, &n))
                }
                "matches" => {
                    check_arity("matches", args, 2)?;
                    let t = eval_to_value(&args[0], state)?;
                    let p = eval_to_value(&args[1], state)?;
                    value_matches(&t, &p)
                }
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
        Expr::FunctionCall(name, args) => {
            // Functions are inherently boolean, but we may be asked to produce
            // a Value here (e.g. as a sub-expression of an outer operator).
            // Wrap the boolean result. `len()` is special-cased to return a
            // Number — the only function that's historically useful as a
            // Value (e.g. `len(path) > 0`).
            match name.as_str() {
                "empty" => {
                    check_arity("empty", args, 1)?;
                    let v = eval_to_value(&args[0], state)?;
                    Ok(Value::Bool(is_empty(&v)))
                }
                "exists" => {
                    check_arity("exists", args, 1)?;
                    let v = eval_to_value(&args[0], state)?;
                    Ok(Value::Bool(!v.is_null()))
                }
                "len" => {
                    check_arity("len", args, 1)?;
                    let v = eval_to_value(&args[0], state)?;
                    Ok(Value::Number(get_length(&v).into()))
                }
                "contains" | "not_contains" | "matches" => {
                    // Evaluate as boolean via eval_ast — already handles arity + args
                    Ok(Value::Bool(eval_ast(expr, state)?))
                }
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
    state.resolve_path(&path_str).map_err(|e| {
        trace!(
            path = %path_str,
            error = %e,
            "Path resolution failed"
        );
        e
    })
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
