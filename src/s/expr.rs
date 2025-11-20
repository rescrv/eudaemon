//! s-expression parser and AST

use crate::s::error::{SError, SResult};

#[derive(Debug, PartialEq, Clone)]
pub enum SExpr {
    Atom(String),
    List(Vec<SExpr>),
}

impl std::fmt::Display for SExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SExpr::Atom(s) => write!(f, "{}", s),
            SExpr::List(l) => {
                write!(f, "(")?;
                for (i, expr) in l.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", expr)?;
                }
                write!(f, ")")
            }
        }
    }
}

pub struct Parser<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        Parser {
            input: input.as_bytes(),
            position: 0,
        }
    }

    pub fn parse(&mut self) -> SResult<SExpr> {
        self.consume_whitespace();
        if self.position >= self.input.len() {
            return Err(SError::new("parse")
                .with_code("unexpected-eof")
                .with_message("Unexpected end of input")
                .with_atom_field("position", self.position));
        }
        match self.peek_char() {
            Some(b'\'') => self.parse_quoted(),
            Some(b'(') => self.parse_list(),
            Some(b'"') => self.parse_quoted_string(),
            Some(_) => self.parse_atom(),
            None => Err(SError::new("parse")
                .with_code("unexpected-eof")
                .with_message("Unexpected end of input")
                .with_atom_field("position", self.position)),
        }
    }

    fn parse_quoted(&mut self) -> SResult<SExpr> {
        self.next_char(); // consume the '
        let expr = self.parse()?;
        Ok(SExpr::List(vec![SExpr::Atom("quote".to_string()), expr]))
    }

    fn peek_char(&self) -> Option<u8> {
        if self.position < self.input.len() {
            Some(self.input[self.position])
        } else {
            None
        }
    }

    fn next_char(&mut self) -> Option<u8> {
        if self.position < self.input.len() {
            let ch = self.input[self.position];
            self.position += 1;
            Some(ch)
        } else {
            None
        }
    }

    fn consume_whitespace(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_whitespace() {
                self.next_char();
            } else {
                break;
            }
        }
    }

    fn parse_list(&mut self) -> SResult<SExpr> {
        let start_position = self.position;
        self.next_char(); // consume '('
        let mut list = Vec::new();
        loop {
            self.consume_whitespace();
            match self.peek_char() {
                Some(b')') => {
                    self.next_char();
                    return Ok(SExpr::List(list));
                }
                Some(_) => {
                    list.push(self.parse()?);
                }
                None => {
                    return Err(SError::new("parse")
                        .with_code("unclosed-list")
                        .with_message("Unclosed list: expected ')' but reached end of input")
                        .with_atom_field("start_position", start_position)
                        .with_atom_field("current_position", self.position)
                        .with_atom_field("list_elements_parsed", list.len()));
                }
            }
        }
    }

    fn parse_quoted_string(&mut self) -> SResult<SExpr> {
        let start_position = self.position;
        self.next_char(); // consume opening '"'
        let mut result = String::from("\"");

        loop {
            match self.peek_char() {
                None => {
                    return Err(SError::new("parse")
                        .with_code("unclosed-string")
                        .with_message(
                            "Unclosed quoted string: expected '\"' but reached end of input",
                        )
                        .with_atom_field("start_position", start_position)
                        .with_atom_field("current_position", self.position));
                }
                Some(b'\\') => {
                    self.next_char(); // consume '\'
                    match self.peek_char() {
                        Some(ch) => {
                            result.push('\\');
                            result.push(ch as char);
                            self.next_char();
                        }
                        None => {
                            return Err(SError::new("parse")
                                .with_code("unclosed-string")
                                .with_message("Unclosed quoted string: escape at end of input")
                                .with_atom_field("start_position", start_position)
                                .with_atom_field("current_position", self.position));
                        }
                    }
                }
                Some(b'"') => {
                    result.push('"');
                    self.next_char(); // consume closing '"'
                    return Ok(SExpr::Atom(result));
                }
                Some(ch) => {
                    result.push(ch as char);
                    self.next_char();
                }
            }
        }
    }

    fn parse_atom(&mut self) -> SResult<SExpr> {
        let start = self.position;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_whitespace() || ch == b'(' || ch == b')' {
                break;
            }
            self.next_char();
        }
        let end = self.position;
        let atom = &self.input[start..end];
        String::from_utf8(atom.to_vec())
            .map(SExpr::Atom)
            .map_err(|e| {
                SError::new("parse")
                    .with_code("invalid-utf8")
                    .with_message("Invalid UTF-8 in atom")
                    .with_atom_field("start_position", start)
                    .with_atom_field("end_position", end)
                    .with_string_field("utf8_error", &e.to_string())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_atom() {
        let mut parser = Parser::new("foo");
        assert_eq!(parser.parse(), Ok(SExpr::Atom("foo".to_string())));
    }

    #[test]
    fn test_simple_list() {
        let mut parser = Parser::new("(foo bar)");
        assert_eq!(
            parser.parse(),
            Ok(SExpr::List(vec![
                SExpr::Atom("foo".to_string()),
                SExpr::Atom("bar".to_string())
            ]))
        );
    }

    #[test]
    fn test_nested_list() {
        let mut parser = Parser::new("(foo (bar baz) qux)");
        assert_eq!(
            parser.parse(),
            Ok(SExpr::List(vec![
                SExpr::Atom("foo".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("bar".to_string()),
                    SExpr::Atom("baz".to_string())
                ]),
                SExpr::Atom("qux".to_string())
            ]))
        );
    }

    #[test]
    fn test_display_atom() {
        let expr = SExpr::Atom("foo".to_string());
        assert_eq!(expr.to_string(), "foo");
    }

    #[test]
    fn test_display_list() {
        let expr = SExpr::List(vec![
            SExpr::Atom("foo".to_string()),
            SExpr::Atom("bar".to_string()),
        ]);
        assert_eq!(expr.to_string(), "(foo bar)");
    }

    #[test]
    fn test_round_trip() {
        let input = "(foo (bar baz) qux)";
        let mut parser = Parser::new(input);
        let ast = parser.parse().unwrap();
        assert_eq!(ast.to_string(), input);
    }

    #[test]
    fn test_empty_list() {
        let mut parser = Parser::new("()");
        assert_eq!(parser.parse(), Ok(SExpr::List(vec![])));
    }

    #[test]
    fn test_list_with_whitespace() {
        let mut parser = Parser::new(" ( foo   bar ) ");
        let ast = parser.parse().unwrap();
        assert_eq!(
            ast,
            SExpr::List(vec![
                SExpr::Atom("foo".to_string()),
                SExpr::Atom("bar".to_string())
            ])
        );
        assert_eq!(ast.to_string(), "(foo bar)");
    }

    #[test]
    fn test_unclosed_list() {
        let mut parser = Parser::new("(foo bar");
        assert!(parser.parse().is_err());
    }

    #[test]
    fn quoted_string_simple() {
        let mut parser = Parser::new(r#""hello""#);
        assert_eq!(parser.parse(), Ok(SExpr::Atom(r#""hello""#.to_string())));
    }

    #[test]
    fn quoted_string_with_spaces() {
        let mut parser = Parser::new(r#""hello world""#);
        assert_eq!(
            parser.parse(),
            Ok(SExpr::Atom(r#""hello world""#.to_string()))
        );
    }

    #[test]
    fn quoted_string_with_escapes() {
        let mut parser = Parser::new(r#""hello \"world\"""#);
        assert_eq!(
            parser.parse(),
            Ok(SExpr::Atom(r#""hello \"world\"""#.to_string()))
        );
    }

    #[test]
    fn list_with_quoted_strings() {
        let mut parser = Parser::new(r#"(obj ("key" "value with spaces"))"#);
        assert_eq!(
            parser.parse(),
            Ok(SExpr::List(vec![
                SExpr::Atom("obj".to_string()),
                SExpr::List(vec![
                    SExpr::Atom(r#""key""#.to_string()),
                    SExpr::Atom(r#""value with spaces""#.to_string())
                ])
            ]))
        );
    }

    #[test]
    fn unclosed_quoted_string() {
        let mut parser = Parser::new(r#""hello"#);
        assert!(parser.parse().is_err());
    }
}
