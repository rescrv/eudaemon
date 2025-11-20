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
