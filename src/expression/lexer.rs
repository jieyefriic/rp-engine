//! Lexer for expression language

use crate::error::ExprError;

/// Token types for the expression language
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    String(String),
    Number(f64),
    True,
    False,
    Null,

    // Identifiers and paths
    Ident(String),

    // Operators
    Eq,        // ==
    Ne,        // !=
    Gt,        // >
    Lt,        // <
    Ge,        // >=
    Le,        // <=
    And,       // &&
    Or,        // ||
    Not,       // !

    // Delimiters
    LParen,    // (
    RParen,    // )
    LBracket,  // [
    RBracket,  // ]
    Dot,       // .
    Comma,     // ,

    // End of input
    Eof,
}

/// Lexer for tokenizing expression strings
pub struct Lexer<'a> {
    input: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    current_pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            chars: input.char_indices().peekable(),
            current_pos: 0,
        }
    }

    /// Tokenize the entire input
    pub fn tokenize(&mut self) -> Result<Vec<Token>, ExprError> {
        let mut tokens = Vec::new();

        loop {
            let token = self.next_token()?;
            if token == Token::Eof {
                tokens.push(token);
                break;
            }
            tokens.push(token);
        }

        Ok(tokens)
    }

    fn next_token(&mut self) -> Result<Token, ExprError> {
        self.skip_whitespace();

        match self.peek_char() {
            None => Ok(Token::Eof),
            Some(c) => match c {
                // String literals
                '"' | '\'' => self.read_string(),

                // Numbers
                '0'..='9' => self.read_number(),

                // Identifiers and keywords
                'a'..='z' | 'A'..='Z' | '_' => self.read_ident(),

                // Operators and delimiters
                '=' => {
                    self.advance();
                    if self.peek_char() == Some('=') {
                        self.advance();
                        Ok(Token::Eq)
                    } else {
                        Err(ExprError::UnexpectedToken("=".into()))
                    }
                }
                '!' => {
                    self.advance();
                    if self.peek_char() == Some('=') {
                        self.advance();
                        Ok(Token::Ne)
                    } else {
                        Ok(Token::Not)
                    }
                }
                '>' => {
                    self.advance();
                    if self.peek_char() == Some('=') {
                        self.advance();
                        Ok(Token::Ge)
                    } else {
                        Ok(Token::Gt)
                    }
                }
                '<' => {
                    self.advance();
                    if self.peek_char() == Some('=') {
                        self.advance();
                        Ok(Token::Le)
                    } else {
                        Ok(Token::Lt)
                    }
                }
                '&' => {
                    self.advance();
                    if self.peek_char() == Some('&') {
                        self.advance();
                        Ok(Token::And)
                    } else {
                        Err(ExprError::UnexpectedToken("&".into()))
                    }
                }
                '|' => {
                    self.advance();
                    if self.peek_char() == Some('|') {
                        self.advance();
                        Ok(Token::Or)
                    } else {
                        Err(ExprError::UnexpectedToken("|".into()))
                    }
                }
                '(' => {
                    self.advance();
                    Ok(Token::LParen)
                }
                ')' => {
                    self.advance();
                    Ok(Token::RParen)
                }
                '[' => {
                    self.advance();
                    Ok(Token::LBracket)
                }
                ']' => {
                    self.advance();
                    Ok(Token::RBracket)
                }
                '.' => {
                    self.advance();
                    Ok(Token::Dot)
                }
                ',' => {
                    self.advance();
                    Ok(Token::Comma)
                }
                _ => Err(ExprError::UnexpectedToken(c.to_string())),
            }
        }
    }

    fn peek_char(&mut self) -> Option<char> {
        self.chars.peek().map(|(_, c)| *c)
    }

    fn advance(&mut self) -> Option<char> {
        self.chars.next().map(|(pos, c)| {
            self.current_pos = pos + c.len_utf8();
            c
        })
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn read_string(&mut self) -> Result<Token, ExprError> {
        let quote = self.advance().unwrap();
        let mut s = String::new();

        loop {
            match self.advance() {
                None => return Err(ExprError::UnexpectedEnd),
                Some(c) if c == quote => break,
                Some('\\') => {
                    match self.advance() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some(c) => s.push(c),
                        None => return Err(ExprError::UnexpectedEnd),
                    }
                }
                Some(c) => s.push(c),
            }
        }

        Ok(Token::String(s))
    }

    fn read_number(&mut self) -> Result<Token, ExprError> {
        let start = self.current_pos;

        while let Some(c) = self.peek_char() {
            if c.is_ascii_digit() || c == '.' {
                self.advance();
            } else {
                break;
            }
        }

        let s = &self.input[start..self.current_pos];
        s.parse::<f64>()
            .map(Token::Number)
            .map_err(|_| ExprError::ParseError(format!("invalid number: {}", s)))
    }

    fn read_ident(&mut self) -> Result<Token, ExprError> {
        let start = self.current_pos;

        while let Some(c) = self.peek_char() {
            if c.is_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }

        let s = &self.input[start..self.current_pos];
        match s {
            "true" => Ok(Token::True),
            "false" => Ok(Token::False),
            "null" => Ok(Token::Null),
            _ => Ok(Token::Ident(s.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_tokens() {
        let mut lexer = Lexer::new("a == b");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens, vec![
            Token::Ident("a".into()),
            Token::Eq,
            Token::Ident("b".into()),
            Token::Eof,
        ]);
    }

    #[test]
    fn test_complex_expression() {
        let mut lexer = Lexer::new("think.done == true && !empty(results)");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens.len(), 12);
    }

    #[test]
    fn test_path_with_selector() {
        let mut lexer = Lexer::new("node[0].field");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens, vec![
            Token::Ident("node".into()),
            Token::LBracket,
            Token::Number(0.0),
            Token::RBracket,
            Token::Dot,
            Token::Ident("field".into()),
            Token::Eof,
        ]);
    }
}
