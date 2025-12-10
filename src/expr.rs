//! s-expression parser and AST

use crate::s::error::{SError, SResult};

/// A symbolic expression: the fundamental data structure for representing structured data.
///
/// S-expressions provide a uniform representation for both code and data, enabling
/// homoiconic transformations where programs can manipulate other programs as data.
///
/// # Examples
///
/// ```
/// use agentkb::SExpr;
///
/// // An atom representing a symbol
/// let symbol = SExpr::Atom("hello".to_string());
///
/// // A list representing a function call
/// let call = SExpr::List(vec![
///     SExpr::Atom("add".to_string()),
///     SExpr::Atom("1".to_string()),
///     SExpr::Atom("2".to_string()),
/// ]);
/// assert_eq!(call.to_string(), "(add 1 2)");
/// ```
#[derive(Debug, PartialEq, Clone)]
pub enum SExpr {
    /// An atomic value: a symbol, number, string, or other indivisible token.
    Atom(String),
    /// A list of S-expressions, where the first element conventionally names the form.
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

/// A recursive descent parser for S-expressions.
///
/// Transforms a string representation into an [`SExpr`] abstract syntax tree.
/// The parser handles atoms, quoted strings with escape sequences, nested lists,
/// and the quote shorthand (`'expr` → `(quote expr)`).
///
/// # Examples
///
/// ```
/// use agentkb::Parser;
///
/// let mut parser = Parser::new("(doc (h1 \"Hello\"))");
/// let expr = parser.parse().unwrap();
/// assert_eq!(expr.to_string(), "(doc (h1 \"Hello\"))");
/// ```
pub struct Parser<'a> {
    input: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    position: usize,
}

impl<'a> Parser<'a> {
    /// Creates a new parser for the given input string.
    pub fn new(input: &'a str) -> Self {
        Parser {
            input,
            chars: input.char_indices().peekable(),
            position: 0,
        }
    }

    /// Parses the input and returns the resulting S-expression.
    ///
    /// # Errors
    ///
    /// Returns an error if the input contains:
    /// - Unclosed lists (missing `)`)
    /// - Unclosed quoted strings (missing `"`)
    /// - Unexpected end of input
    pub fn parse(&mut self) -> SResult<SExpr> {
        self.consume_whitespace();
        match self.peek_char() {
            Some('\'') => self.parse_quoted(),
            Some('(') => self.parse_list(),
            Some('"') => self.parse_quoted_string(),
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

    fn peek_char(&mut self) -> Option<char> {
        self.chars.peek().map(|&(_, ch)| ch)
    }

    fn next_char(&mut self) -> Option<char> {
        if let Some((idx, ch)) = self.chars.next() {
            self.position = idx + ch.len_utf8();
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
                Some(')') => {
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
                Some('\\') => {
                    self.next_char(); // consume '\'
                    match self.peek_char() {
                        Some(ch) => {
                            result.push('\\');
                            result.push(ch);
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
                Some('"') => {
                    result.push('"');
                    self.next_char(); // consume closing '"'
                    return Ok(SExpr::Atom(result));
                }
                Some(ch) => {
                    result.push(ch);
                    self.next_char();
                }
            }
        }
    }

    fn parse_atom(&mut self) -> SResult<SExpr> {
        let start = self.position;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_whitespace() || ch == '(' || ch == ')' {
                break;
            }
            self.next_char();
        }
        let end = self.position;
        Ok(SExpr::Atom(self.input[start..end].to_string()))
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
    fn unclosed_list_error() {
        let mut parser = Parser::new("(foo bar");
        let err = parser.parse().unwrap_err();
        assert!(err.to_string().contains("unclosed-list"));
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
    fn unclosed_quoted_string_error() {
        let mut parser = Parser::new(r#""hello"#);
        let err = parser.parse().unwrap_err();
        assert!(err.to_string().contains("unclosed-string"));
    }

    #[test]
    fn empty_input_error() {
        let mut parser = Parser::new("");
        let err = parser.parse().unwrap_err();
        assert!(err.to_string().contains("unexpected-eof"));
    }

    #[test]
    fn whitespace_only_input_error() {
        let mut parser = Parser::new("   \n\t  ");
        let err = parser.parse().unwrap_err();
        assert!(err.to_string().contains("unexpected-eof"));
    }

    #[test]
    fn quoted_syntax_shorthand() {
        let mut parser = Parser::new("'foo");
        assert_eq!(parser.parse().unwrap().to_string(), "(quote foo)");
    }

    #[test]
    fn quoted_list_shorthand() {
        let mut parser = Parser::new("'(a b c)");
        assert_eq!(parser.parse().unwrap().to_string(), "(quote (a b c))");
    }

    #[test]
    fn deeply_nested_lists() {
        let mut parser = Parser::new("(a (b (c (d (e (f))))))");
        assert_eq!(
            parser.parse().unwrap().to_string(),
            "(a (b (c (d (e (f))))))"
        );
    }

    #[test]
    fn atom_with_special_characters() {
        let mut parser = Parser::new("foo-bar_baz123");
        assert_eq!(
            parser.parse().unwrap(),
            SExpr::Atom("foo-bar_baz123".to_string())
        );
    }

    #[test]
    fn atom_with_symbols() {
        let mut parser = Parser::new("+-*/<>=!?");
        assert_eq!(
            parser.parse().unwrap(),
            SExpr::Atom("+-*/<>=!?".to_string())
        );
    }

    #[test]
    fn numeric_atom_positive() {
        let mut parser = Parser::new("123");
        assert_eq!(parser.parse().unwrap(), SExpr::Atom("123".to_string()));
    }

    #[test]
    fn numeric_atom_negative() {
        let mut parser = Parser::new("-456");
        assert_eq!(parser.parse().unwrap(), SExpr::Atom("-456".to_string()));
    }

    #[test]
    fn numeric_atom_float() {
        let mut parser = Parser::new("3.14159");
        assert_eq!(parser.parse().unwrap(), SExpr::Atom("3.14159".to_string()));
    }

    #[test]
    fn quoted_string_with_newlines() {
        let mut parser = Parser::new(r#""hello\nworld""#);
        assert_eq!(
            parser.parse().unwrap(),
            SExpr::Atom(r#""hello\nworld""#.to_string())
        );
    }

    #[test]
    fn quoted_string_with_tabs() {
        let mut parser = Parser::new(r#""col1\tcol2""#);
        assert_eq!(
            parser.parse().unwrap(),
            SExpr::Atom(r#""col1\tcol2""#.to_string())
        );
    }

    #[test]
    fn quoted_string_with_backslashes() {
        let mut parser = Parser::new(r#""path\\to\\file""#);
        assert_eq!(
            parser.parse().unwrap(),
            SExpr::Atom(r#""path\\to\\file""#.to_string())
        );
    }

    #[test]
    fn escape_at_end_of_string_error() {
        let mut parser = Parser::new(r#""hello\"#);
        let err = parser.parse().unwrap_err();
        assert!(err.to_string().contains("unclosed-string"));
    }

    #[test]
    fn nested_lists_with_multiple_levels() {
        let input = "(doc (h1 \"Title\") (ul (li \"A\") (li \"B\")) (p \"End\"))";
        let mut parser = Parser::new(input);
        assert_eq!(parser.parse().unwrap().to_string(), input);
    }

    #[test]
    fn list_with_mixed_content() {
        let mut parser = Parser::new("(func 123 \"str\" symbol (nested))");
        let expected = SExpr::List(vec![
            SExpr::Atom("func".to_string()),
            SExpr::Atom("123".to_string()),
            SExpr::Atom("\"str\"".to_string()),
            SExpr::Atom("symbol".to_string()),
            SExpr::List(vec![SExpr::Atom("nested".to_string())]),
        ]);
        assert_eq!(parser.parse().unwrap(), expected);
    }

    #[test]
    fn display_nested_list() {
        let expr = SExpr::List(vec![
            SExpr::Atom("outer".to_string()),
            SExpr::List(vec![
                SExpr::Atom("inner".to_string()),
                SExpr::Atom("value".to_string()),
            ]),
        ]);
        assert_eq!(expr.to_string(), "(outer (inner value))");
    }

    #[test]
    fn display_empty_list() {
        let expr = SExpr::List(vec![]);
        assert_eq!(expr.to_string(), "()");
    }

    #[test]
    fn display_single_element_list() {
        let expr = SExpr::List(vec![SExpr::Atom("only".to_string())]);
        assert_eq!(expr.to_string(), "(only)");
    }

    #[test]
    fn parser_handles_carriage_return() {
        let mut parser = Parser::new("(a\r\nb)");
        assert_eq!(
            parser.parse().unwrap(),
            SExpr::List(vec![
                SExpr::Atom("a".to_string()),
                SExpr::Atom("b".to_string()),
            ])
        );
    }

    #[test]
    fn unclosed_nested_list_error() {
        let mut parser = Parser::new("(a (b (c)");
        let err = parser.parse().unwrap_err();
        assert!(err.to_string().contains("unclosed-list"));
    }

    #[test]
    fn quoted_string_empty() {
        let mut parser = Parser::new(r#""""#);
        assert_eq!(parser.parse().unwrap(), SExpr::Atom(r#""""#.to_string()));
    }
}
