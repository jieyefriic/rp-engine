//! Parser for expression language

use crate::error::ExprError;
use super::lexer::Token;
use serde_json::Value;

/// Comparison operators
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompareOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

/// Path segment types
#[derive(Debug, Clone, PartialEq)]
pub enum PathSegment {
    Field(String),
    Index(i32),
    ShortId(String),
    All,
}

/// Path expression
#[derive(Debug, Clone, PartialEq)]
pub struct PathExpr {
    pub segments: Vec<PathSegment>,
}

impl PathExpr {
    pub fn new(segments: Vec<PathSegment>) -> Self {
        Self { segments }
    }

    /// Get the first segment as a field name (node_id or "input")
    pub fn root(&self) -> Option<&str> {
        match self.segments.first() {
            Some(PathSegment::Field(name)) => Some(name),
            _ => None,
        }
    }
}

/// Expression AST
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Logical OR
    Or(Box<Expr>, Box<Expr>),
    /// Logical AND
    And(Box<Expr>, Box<Expr>),
    /// Logical NOT
    Not(Box<Expr>),
    /// Comparison
    Compare(Box<Expr>, CompareOp, Box<Expr>),
    /// Path reference
    Path(PathExpr),
    /// Literal value
    Literal(Value),
    /// Function call
    FunctionCall(String, Box<Expr>),
}

/// Parser for expression language
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    /// Parse the tokens into an AST
    pub fn parse(&mut self) -> Result<Expr, ExprError> {
        let expr = self.parse_or()?;

        if !matches!(self.peek(), Token::Eof) {
            return Err(ExprError::UnexpectedToken(format!("{:?}", self.peek())));
        }

        Ok(expr)
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> &Token {
        let token = self.tokens.get(self.pos).unwrap_or(&Token::Eof);
        self.pos += 1;
        token
    }

    fn parse_or(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.parse_and()?;

        while matches!(self.peek(), Token::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }

        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.parse_not()?;

        while matches!(self.peek(), Token::And) {
            self.advance();
            let right = self.parse_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }

        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, ExprError> {
        if matches!(self.peek(), Token::Not) {
            self.advance();
            let expr = self.parse_not()?;
            Ok(Expr::Not(Box::new(expr)))
        } else {
            self.parse_compare()
        }
    }

    fn parse_compare(&mut self) -> Result<Expr, ExprError> {
        let left = self.parse_value()?;

        let op = match self.peek() {
            Token::Eq => Some(CompareOp::Eq),
            Token::Ne => Some(CompareOp::Ne),
            Token::Gt => Some(CompareOp::Gt),
            Token::Lt => Some(CompareOp::Lt),
            Token::Ge => Some(CompareOp::Ge),
            Token::Le => Some(CompareOp::Le),
            _ => None,
        };

        if let Some(op) = op {
            self.advance();
            let right = self.parse_value()?;
            Ok(Expr::Compare(Box::new(left), op, Box::new(right)))
        } else {
            Ok(left)
        }
    }

    fn parse_value(&mut self) -> Result<Expr, ExprError> {
        match self.peek().clone() {
            Token::LParen => {
                self.advance();
                let expr = self.parse_or()?;
                if !matches!(self.peek(), Token::RParen) {
                    return Err(ExprError::UnexpectedToken("expected )".into()));
                }
                self.advance();
                Ok(expr)
            }
            Token::String(s) => {
                self.advance();
                Ok(Expr::Literal(Value::String(s)))
            }
            Token::Number(n) => {
                self.advance();
                Ok(Expr::Literal(serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)))
            }
            Token::True => {
                self.advance();
                Ok(Expr::Literal(Value::Bool(true)))
            }
            Token::False => {
                self.advance();
                Ok(Expr::Literal(Value::Bool(false)))
            }
            Token::Null => {
                self.advance();
                Ok(Expr::Literal(Value::Null))
            }
            Token::Ident(name) => {
                self.advance();

                // Check if this is a function call
                if matches!(self.peek(), Token::LParen) {
                    self.advance();
                    let arg = self.parse_or()?;
                    if !matches!(self.peek(), Token::RParen) {
                        return Err(ExprError::UnexpectedToken("expected )".into()));
                    }
                    self.advance();
                    Ok(Expr::FunctionCall(name, Box::new(arg)))
                } else {
                    // Parse as path
                    let path = self.parse_path_from_ident(name)?;
                    Ok(Expr::Path(path))
                }
            }
            _ => Err(ExprError::UnexpectedToken(format!("{:?}", self.peek()))),
        }
    }

    fn parse_path_from_ident(&mut self, first: String) -> Result<PathExpr, ExprError> {
        let mut segments = vec![PathSegment::Field(first)];

        loop {
            match self.peek() {
                Token::Dot => {
                    self.advance();
                    match self.peek().clone() {
                        Token::Ident(name) => {
                            self.advance();
                            segments.push(PathSegment::Field(name));
                        }
                        _ => return Err(ExprError::UnexpectedToken("expected identifier after .".into())),
                    }
                }
                Token::LBracket => {
                    self.advance();
                    let segment = self.parse_selector()?;
                    segments.push(segment);
                    if !matches!(self.peek(), Token::RBracket) {
                        return Err(ExprError::UnexpectedToken("expected ]".into()));
                    }
                    self.advance();
                }
                _ => break,
            }
        }

        Ok(PathExpr::new(segments))
    }

    fn parse_selector(&mut self) -> Result<PathSegment, ExprError> {
        match self.peek().clone() {
            Token::Number(n) => {
                self.advance();
                Ok(PathSegment::Index(n as i32))
            }
            Token::Ident(s) => {
                self.advance();
                if s == "*" {
                    Ok(PathSegment::All)
                } else {
                    Ok(PathSegment::ShortId(s))
                }
            }
            // Handle negative index
            _ => {
                // Check for minus sign pattern (not in lexer, but common case)
                Err(ExprError::UnexpectedToken("expected selector".into()))
            }
        }
    }
}

/// Parse an expression string
pub fn parse(tokens: Vec<Token>) -> Result<Expr, ExprError> {
    let mut parser = Parser::new(tokens);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::lexer::Lexer;

    fn parse_expr(input: &str) -> Result<Expr, ExprError> {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize()?;
        let mut parser = Parser::new(tokens);
        parser.parse()
    }

    #[test]
    fn test_simple_comparison() {
        let expr = parse_expr("a == true").unwrap();
        match expr {
            Expr::Compare(left, CompareOp::Eq, right) => {
                assert!(matches!(*left, Expr::Path(_)));
                assert!(matches!(*right, Expr::Literal(Value::Bool(true))));
            }
            _ => panic!("expected comparison"),
        }
    }

    #[test]
    fn test_path_expression() {
        let expr = parse_expr("node.field.subfield").unwrap();
        match expr {
            Expr::Path(path) => {
                assert_eq!(path.segments.len(), 3);
            }
            _ => panic!("expected path"),
        }
    }

    #[test]
    fn test_function_call() {
        let expr = parse_expr("empty(results)").unwrap();
        match expr {
            Expr::FunctionCall(name, _) => {
                assert_eq!(name, "empty");
            }
            _ => panic!("expected function call"),
        }
    }

    #[test]
    fn test_complex_expression() {
        let expr = parse_expr("think.done == true && !empty(results)").unwrap();
        assert!(matches!(expr, Expr::And(_, _)));
    }
}
