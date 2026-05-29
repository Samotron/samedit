//! A tiny arithmetic evaluator for the calculator built-in (M13.4).
//!
//! Supports `+ - * /`, parentheses, and unary minus over decimal numbers,
//! with the usual precedence (`=2+2*3` → `8`). No external dependency — a
//! hand-rolled tokenizer plus a recursive-descent parser. Division by zero
//! and malformed input return `None`, so the provider simply emits no row.

/// Evaluate an arithmetic expression, returning its value or `None` if it is
/// malformed (or divides by zero).
pub fn evaluate(input: &str) -> Option<f64> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let value = parser.expr()?;
    // Reject trailing junk like `2 3` or `2)`.
    if parser.pos != parser.tokens.len() {
        return None;
    }
    value.is_finite().then_some(value)
}

/// Format a result for display: integers without a decimal point, otherwise a
/// trimmed decimal.
pub fn format_result(value: f64) -> String {
    if value == 0.0 {
        // Normalise `-0` to `0`.
        return "0".to_string();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    format!("{value}")
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

fn tokenize(input: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' => {
                tokens.push(Token::Star);
                chars.next();
            }
            '/' => {
                tokens.push(Token::Slash);
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            c if c.is_ascii_digit() || c == '.' => {
                let mut literal = String::new();
                let mut seen_dot = false;
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() {
                        literal.push(c);
                        chars.next();
                    } else if c == '.' && !seen_dot {
                        seen_dot = true;
                        literal.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Num(literal.parse().ok()?));
            }
            _ => return None,
        }
    }
    (!tokens.is_empty()).then_some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.peek();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    /// expr := term (('+' | '-') term)*
    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        while let Some(op @ (Token::Plus | Token::Minus)) = self.peek() {
            self.bump();
            let rhs = self.term()?;
            value = if op == Token::Plus {
                value + rhs
            } else {
                value - rhs
            };
        }
        Some(value)
    }

    /// term := factor (('*' | '/') factor)*
    fn term(&mut self) -> Option<f64> {
        let mut value = self.factor()?;
        while let Some(op @ (Token::Star | Token::Slash)) = self.peek() {
            self.bump();
            let rhs = self.factor()?;
            if op == Token::Slash {
                if rhs == 0.0 {
                    return None;
                }
                value /= rhs;
            } else {
                value *= rhs;
            }
        }
        Some(value)
    }

    /// factor := '-' factor | '(' expr ')' | number
    fn factor(&mut self) -> Option<f64> {
        match self.bump()? {
            Token::Num(n) => Some(n),
            Token::Minus => Some(-self.factor()?),
            Token::Plus => self.factor(),
            Token::LParen => {
                let value = self.expr()?;
                matches!(self.bump(), Some(Token::RParen)).then_some(value)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_precedence() {
        assert_eq!(evaluate("2+2*3"), Some(8.0));
        assert_eq!(evaluate("(2+2)*3"), Some(12.0));
    }

    #[test]
    fn handles_unary_minus_and_whitespace() {
        assert_eq!(evaluate(" -3 + 5 "), Some(2.0));
        assert_eq!(evaluate("-(2*3)"), Some(-6.0));
    }

    #[test]
    fn decimals_and_division() {
        assert_eq!(evaluate("10/4"), Some(2.5));
        assert_eq!(evaluate("1.5*2"), Some(3.0));
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(evaluate(""), None);
        assert_eq!(evaluate("2+"), None);
        assert_eq!(evaluate("2 3"), None);
        assert_eq!(evaluate("(2+3"), None);
        assert_eq!(evaluate("abc"), None);
    }

    #[test]
    fn division_by_zero_is_none() {
        assert_eq!(evaluate("1/0"), None);
    }

    #[test]
    fn formats_integers_without_decimal() {
        assert_eq!(format_result(8.0), "8");
        assert_eq!(format_result(2.5), "2.5");
        assert_eq!(format_result(-0.0), "0");
    }
}
