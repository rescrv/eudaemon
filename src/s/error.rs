//! Structured S-expression error utilities.

use std::fmt;

use crate::s::expr::SExpr;

/// Result alias that carries structured `SExpr` errors.
pub type SResult<T> = Result<T, SError>;

/// Structured error that serializes as an S-expression for downstream processing.
#[derive(Debug, Clone, PartialEq)]
pub struct SError {
    detail: SExpr,
}

impl SError {
    /// Builds a new error anchored to a specific phase of processing.
    pub fn new(phase: &str) -> Self {
        SError {
            detail: SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                field("phase", atom(phase)),
            ]),
        }
    }

    /// Annotates the error with a machine-readable code.
    pub fn with_code(self, code: &str) -> Self {
        self.with_field("code", atom(code))
    }

    /// Adds a human-readable message to the error.
    pub fn with_message(self, message: &str) -> Self {
        self.with_field("message", string_literal(message))
    }

    /// Adds an arbitrary field with an atomic value.
    pub fn with_atom_field<T: ToString>(self, name: &str, value: T) -> Self {
        self.with_field(name, atom(value))
    }

    /// Adds an arbitrary field with a string literal value.
    pub fn with_string_field(self, name: &str, value: &str) -> Self {
        self.with_field(name, string_literal(value))
    }

    /// Adds an arbitrary field expressed as an `SExpr`.
    pub fn with_field(mut self, name: &str, value: SExpr) -> Self {
        if let SExpr::List(entries) = &mut self.detail {
            entries.push(field(name, value));
        }
        self
    }

    /// Returns the underlying detail `SExpr`.
    pub fn detail(&self) -> &SExpr {
        &self.detail
    }

    /// Consumes the error and returns the inner `SExpr` detail.
    pub fn into_detail(self) -> SExpr {
        self.detail
    }
}

impl fmt::Display for SError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.detail)
    }
}

impl std::error::Error for SError {}

impl From<SExpr> for SError {
    fn from(detail: SExpr) -> Self {
        SError { detail }
    }
}

fn field(name: &str, value: SExpr) -> SExpr {
    SExpr::List(vec![SExpr::Atom(name.to_string()), value])
}

fn atom<T: ToString>(value: T) -> SExpr {
    SExpr::Atom(value.to_string())
}

fn string_literal(value: &str) -> SExpr {
    SExpr::Atom(format!("\"{}\"", escape_string(value)))
}

fn escape_string(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_new_creates_base_structure() {
        let err = SError::new("test-phase");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test-phase".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_with_code() {
        let err = SError::new("parse").with_code("syntax-error");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("parse".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("syntax-error".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_with_message() {
        let err = SError::new("eval").with_message("Something went wrong");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("eval".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"Something went wrong\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_with_atom_field() {
        let err = SError::new("test").with_atom_field("count", 42);
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("count".to_string()),
                    SExpr::Atom("42".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_with_string_field() {
        let err = SError::new("test").with_string_field("name", "value");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("name".to_string()),
                    SExpr::Atom("\"value\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_with_field_sexpr() {
        let field_value = SExpr::List(vec![
            SExpr::Atom("nested".to_string()),
            SExpr::Atom("data".to_string()),
        ]);
        let err = SError::new("test").with_field("complex", field_value.clone());
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test".to_string()),
                ]),
                SExpr::List(vec![SExpr::Atom("complex".to_string()), field_value,]),
            ])
        );
    }

    #[test]
    fn error_chained_builders() {
        let err = SError::new("mutations")
            .with_code("index-out-of-bounds")
            .with_message("Path index exceeds list length")
            .with_atom_field("index", 10)
            .with_atom_field("list_length", 5);
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("mutations".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("index-out-of-bounds".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"Path index exceeds list length\"".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("index".to_string()),
                    SExpr::Atom("10".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("list_length".to_string()),
                    SExpr::Atom("5".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_into_detail_consumes() {
        let err = SError::new("test").with_code("test-code");
        let expected = err.detail().clone();
        let detail = err.into_detail();
        assert_eq!(detail, expected);
    }

    #[test]
    fn error_from_sexpr() {
        let sexpr = SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom("custom".to_string()),
        ]);
        let err: SError = sexpr.clone().into();
        assert_eq!(*err.detail(), sexpr);
    }

    #[test]
    fn error_display_matches_detail() {
        let err = SError::new("test").with_code("code");
        assert_eq!(err.to_string(), err.detail().to_string());
    }

    #[test]
    fn error_escapes_newline_in_message() {
        let err = SError::new("test").with_message("line1\nline2");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"line1\\nline2\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_escapes_tab_in_message() {
        let err = SError::new("test").with_message("col1\tcol2");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"col1\\tcol2\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_escapes_quotes_in_message() {
        let err = SError::new("test").with_message("has \"quotes\"");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"has \\\"quotes\\\"\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_escapes_backslash_in_message() {
        let err = SError::new("test").with_message("path\\to\\file");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"path\\\\to\\\\file\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_escapes_carriage_return_in_message() {
        let err = SError::new("test").with_message("line1\rline2");
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("test".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"line1\\rline2\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn error_implements_std_error() {
        let err = SError::new("test");
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn error_clone() {
        let err = SError::new("test").with_code("code");
        let cloned = err.clone();
        assert_eq!(err.detail(), cloned.detail());
    }

    #[test]
    fn error_partial_eq() {
        let err1 = SError::new("test").with_code("code");
        let err2 = SError::new("test").with_code("code");
        let err3 = SError::new("test").with_code("different");
        assert_eq!(err1, err2);
        assert_ne!(err1, err3);
    }
}
