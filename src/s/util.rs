//! Shared utility functions for S-expression string handling.
//!
//! This module provides common string manipulation functions used throughout
//! the S-expression processing pipeline.

use super::expr::SExpr;
use super::json::unescape_string;

/// Escapes special characters in a string for S-expression representation.
///
/// Handles backslash, double-quote, newline, carriage return, and tab.
pub fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Creates a quoted string atom with proper escaping.
///
/// Wraps the escaped string in double quotes to create a valid S-expression string atom.
pub fn string_atom(s: &str) -> SExpr {
    SExpr::Atom(format!("\"{}\"", escape_string(s)))
}

/// Extracts string content from an atom, handling quoted strings.
///
/// If the atom is a quoted string (starts and ends with `"`), unescapes and returns
/// the inner content. Otherwise returns the atom value as-is.
/// Returns an empty string for non-atom expressions.
pub fn extract_string(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                unescape_string(&s[1..s.len() - 1])
            } else {
                s.clone()
            }
        }
        _ => String::new(),
    }
}

/// Extracts plain text content from an S-expression node recursively.
///
/// For atoms, returns the unescaped string content.
/// For lists, concatenates the text content of all children (skipping the tag).
pub fn extract_text_content(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                unescape_string(&s[1..s.len() - 1])
            } else {
                s.clone()
            }
        }
        SExpr::List(items) => {
            let mut text = String::new();
            for item in items.iter().skip(1) {
                text.push_str(&extract_text_content(item));
            }
            text
        }
    }
}

/// Determines if a URL is an internal link.
///
/// Returns true for:
/// - Empty URLs
/// - URLs starting with `#` (anchor links)
/// - URLs starting with `./` or `../` (relative paths)
/// - URLs without a protocol (no `://`)
pub fn is_internal_url(url: &str) -> bool {
    if url.is_empty() || url.starts_with('#') {
        return true;
    }
    if url.starts_with("./") || url.starts_with("../") {
        return true;
    }
    !url.contains("://")
}

/// Gets the path of the previous sibling, given a path's indices.
///
/// Returns `None` if the path is empty or the last index is 0 (no previous sibling).
pub fn get_previous_sibling_indices(indices: &[usize]) -> Option<Vec<usize>> {
    if indices.is_empty() {
        return None;
    }
    let last = *indices.last()?;
    if last == 0 {
        return None;
    }
    let mut new_indices = indices.to_vec();
    *new_indices.last_mut()? = last - 1;
    Some(new_indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_string_backslash() {
        assert_eq!(escape_string("a\\b"), "a\\\\b");
    }

    #[test]
    fn escape_string_quote() {
        assert_eq!(escape_string("a\"b"), "a\\\"b");
    }

    #[test]
    fn escape_string_newline() {
        assert_eq!(escape_string("a\nb"), "a\\nb");
    }

    #[test]
    fn escape_string_carriage_return() {
        assert_eq!(escape_string("a\rb"), "a\\rb");
    }

    #[test]
    fn escape_string_tab() {
        assert_eq!(escape_string("a\tb"), "a\\tb");
    }

    #[test]
    fn escape_string_combined() {
        assert_eq!(escape_string("a\"\n\\b"), "a\\\"\\n\\\\b");
    }

    #[test]
    fn string_atom_basic() {
        assert_eq!(string_atom("hello"), SExpr::Atom("\"hello\"".to_string()));
    }

    #[test]
    fn string_atom_with_escapes() {
        assert_eq!(
            string_atom("hello\nworld"),
            SExpr::Atom("\"hello\\nworld\"".to_string())
        );
    }

    #[test]
    fn extract_string_quoted() {
        let atom = SExpr::Atom("\"hello\"".to_string());
        assert_eq!(extract_string(&atom), "hello");
    }

    #[test]
    fn extract_string_unquoted() {
        let atom = SExpr::Atom("unquoted".to_string());
        assert_eq!(extract_string(&atom), "unquoted");
    }

    #[test]
    fn extract_string_with_escapes() {
        let atom = SExpr::Atom("\"hello\\nworld\"".to_string());
        assert_eq!(extract_string(&atom), "hello\nworld");
    }

    #[test]
    fn extract_string_from_list() {
        let list = SExpr::List(vec![SExpr::Atom("tag".to_string())]);
        assert_eq!(extract_string(&list), "");
    }

    #[test]
    fn extract_text_content_atom() {
        let atom = SExpr::Atom("\"hello\"".to_string());
        assert_eq!(extract_text_content(&atom), "hello");
    }

    #[test]
    fn extract_text_content_nested() {
        let expr = SExpr::List(vec![
            SExpr::Atom("p".to_string()),
            SExpr::Atom("\"Hello \"".to_string()),
            SExpr::List(vec![
                SExpr::Atom("strong".to_string()),
                SExpr::Atom("\"world\"".to_string()),
            ]),
            SExpr::Atom("\"!\"".to_string()),
        ]);
        assert_eq!(extract_text_content(&expr), "Hello world!");
    }

    #[test]
    fn is_internal_url_empty() {
        assert!(is_internal_url(""));
    }

    #[test]
    fn is_internal_url_anchor() {
        assert!(is_internal_url("#section"));
    }

    #[test]
    fn is_internal_url_relative_dot() {
        assert!(is_internal_url("./page.md"));
    }

    #[test]
    fn is_internal_url_relative_dotdot() {
        assert!(is_internal_url("../other/page.md"));
    }

    #[test]
    fn is_internal_url_no_protocol() {
        assert!(is_internal_url("page.md"));
    }

    #[test]
    fn is_internal_url_https_external() {
        assert!(!is_internal_url("https://example.com"));
    }

    #[test]
    fn is_internal_url_http_external() {
        assert!(!is_internal_url("http://example.com"));
    }

    #[test]
    fn get_previous_sibling_indices_empty() {
        assert!(get_previous_sibling_indices(&[]).is_none());
    }

    #[test]
    fn get_previous_sibling_indices_first_child() {
        assert!(get_previous_sibling_indices(&[1, 0]).is_none());
    }

    #[test]
    fn get_previous_sibling_indices_valid() {
        assert_eq!(
            get_previous_sibling_indices(&[1, 2, 3]),
            Some(vec![1, 2, 2])
        );
    }
}
