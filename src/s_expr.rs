//! s-expression parser and AST

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

    pub fn parse(&mut self) -> Result<SExpr, String> {
        self.consume_whitespace();
        if self.position >= self.input.len() {
            return Err("Unexpected end of input".to_string());
        }
        match self.peek_char() {
            Some(b'(') => self.parse_list(),
            Some(_) => self.parse_atom(),
            None => Err("Unexpected end of input".to_string()),
        }
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

    fn parse_list(&mut self) -> Result<SExpr, String> {
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
                None => return Err("Unclosed list".to_string()),
            }
        }
    }

    fn parse_atom(&mut self) -> Result<SExpr, String> {
        let start = self.position;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_whitespace() || ch == b'(' || ch == b')' {
                break;
            }
            self.next_char();
        }
        let end = self.position;
        let atom = &self.input[start..end];
        Ok(SExpr::Atom(
            String::from_utf8(atom.to_vec()).map_err(|e| e.to_string())?,
        ))
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
}
