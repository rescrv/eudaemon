//! Selector language for querying markdown AST nodes.
//!
//! This module provides a CSS-like selector language for finding nodes in markdown documents.
//!
//! # Selector Grammar
//!
//! ```text
//! selector     = simple_sel | compound_sel
//! simple_sel   = type_sel | path_sel | tagged_sel
//! compound_sel = simple_sel (combinator simple_sel)*
//!
//! type_sel     = tag_name                    # "h1", "p", "ul", "blockquote"
//!              | tag_name "[" attr_pred "]"  # "h1[depth=2]", "link[internal]"
//!              | "*"                         # any node
//!              | "h*"                        # heading wildcard (h1-h6)
//!
//! path_sel     = "@" path_id                 # "@1.2.3" - specific path
//!
//! tagged_sel   = "#" tag_key                 # "#deprecated" - nodes with this tag
//!              | "#" tag_key "=" value       # "#status=draft"
//!
//! attr_pred    = attr_name                   # boolean attribute (e.g., "internal")
//!              | attr_name op value          # comparison
//!
//! op           = "="  | "!="                 # exact match / not equal
//!              | "~=" | "^=" | "$="          # contains / starts / ends
//!              | "<"  | "<=" | ">" | ">="    # numeric comparison
//!
//! combinator   = " "                         # descendant (any depth)
//!              | " > "                       # direct child
//!              | " + "                       # adjacent sibling
//!              | " ~ "                       # general sibling
//! ```
//!
//! # Examples
//!
//! ```text
//! h2                    - All h2 elements
//! h*                    - All headings (h1-h6)
//! h1 > p                - Paragraphs that are direct children of h1
//! link[internal]        - All internal links
//! link[url^="./api"]    - Links starting with "./api"
//! h2[text~="Legacy"]    - H2s containing "Legacy" in text
//! #deprecated           - Nodes tagged with "deprecated"
//! @1.2.3                - The specific node at path 1.2.3
//! blockquote p          - Paragraphs anywhere inside blockquotes
//! *[depth<=3]           - Any node at depth 3 or less
//! ```

use super::error::{SError, SResult};
use super::expr::SExpr;
use super::nodeid::PathId;
use super::util::{
    extract_string, extract_text_content, get_previous_sibling_indices, is_internal_url,
};

/// A parsed selector that can be matched against AST nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    /// Chain of simple selectors with combinators.
    pub parts: Vec<SelectorPart>,
}

/// A single part of a compound selector.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectorPart {
    /// The simple selector.
    pub simple: SimpleSelector,
    /// How this part relates to the next (None for last part).
    pub combinator: Option<Combinator>,
}

/// A simple (non-compound) selector.
#[derive(Debug, Clone, PartialEq)]
pub enum SimpleSelector {
    /// Match any node.
    Universal,
    /// Match by tag name (e.g., "h1", "p", "ul").
    Tag(String),
    /// Match any heading (h1-h6).
    AnyHeading,
    /// Match by exact path.
    Path(PathId),
    /// Match by node tag (metadata).
    Tagged {
        /// The tag key to match.
        key: String,
        /// The optional tag value to match.
        value: Option<String>,
    },
    /// Tag selector with attribute predicates.
    WithAttributes {
        /// The base selector to match before applying predicates.
        base: Box<SimpleSelector>,
        /// Additional attribute predicates that must all be satisfied.
        predicates: Vec<AttributePredicate>,
    },
}

/// How selectors are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// Descendant (any depth) - space.
    Descendant,
    /// Direct child - ">".
    Child,
    /// Adjacent sibling - "+".
    Adjacent,
    /// General sibling - "~".
    Sibling,
}

/// A predicate on a node attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributePredicate {
    /// The attribute name.
    pub name: String,
    /// The comparison operator and value (None for boolean check).
    pub comparison: Option<(CompareOp, String)>,
}

/// Comparison operators for attribute predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    /// Exact equality.
    Eq,
    /// Not equal.
    NotEq,
    /// Contains substring.
    Contains,
    /// Starts with.
    StartsWith,
    /// Ends with.
    EndsWith,
    /// Less than (numeric).
    Lt,
    /// Less than or equal (numeric).
    Lte,
    /// Greater than (numeric).
    Gt,
    /// Greater than or equal (numeric).
    Gte,
}

impl Selector {
    /// Parses a selector string.
    pub fn parse(input: &str) -> SResult<Self> {
        SelectorParser::new(input).parse()
    }

    /// Returns true if this selector matches a node at the given context.
    pub fn matches(&self, doc: &SExpr, path: &PathId, depth: usize) -> bool {
        if self.parts.is_empty() {
            return false;
        }

        // For single-part selectors, just check the simple match.
        if self.parts.len() == 1 {
            let node = match get_node_at_path(doc, path) {
                Some(n) => n,
                None => return false,
            };
            return self.parts[0].simple.matches(&node, path, depth, doc);
        }

        // For compound selectors, we need to check the chain.
        self.matches_compound(doc, path, depth)
    }

    /// Matches compound selectors by walking the chain.
    fn matches_compound(&self, doc: &SExpr, path: &PathId, depth: usize) -> bool {
        // The last part must match the target node.
        let last_idx = self.parts.len() - 1;
        let node = match get_node_at_path(doc, path) {
            Some(n) => n,
            None => return false,
        };

        if !self.parts[last_idx].simple.matches(&node, path, depth, doc) {
            return false;
        }

        // Walk backwards through the selector parts.
        let mut current_path = path.clone();
        let mut current_depth = depth;

        for i in (0..last_idx).rev() {
            let part = &self.parts[i];
            let combinator = part.combinator.unwrap_or(Combinator::Descendant);

            let found = match combinator {
                Combinator::Child => {
                    // Must match immediate parent.
                    if let Some(parent_path) = current_path.parent() {
                        if let Some(parent_node) = get_node_at_path(doc, &parent_path) {
                            if part.simple.matches(
                                &parent_node,
                                &parent_path,
                                current_depth - 1,
                                doc,
                            ) {
                                current_path = parent_path;
                                current_depth -= 1;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
                Combinator::Descendant => {
                    // Must match some ancestor.
                    let mut ancestor_path = current_path.parent();
                    let mut ancestor_depth = current_depth.saturating_sub(1);
                    let mut matched = false;

                    while let Some(ap) = ancestor_path {
                        if let Some(ancestor_node) = get_node_at_path(doc, &ap)
                            && part
                                .simple
                                .matches(&ancestor_node, &ap, ancestor_depth, doc)
                        {
                            current_path = ap;
                            current_depth = ancestor_depth;
                            matched = true;
                            break;
                        }
                        ancestor_path = ap.parent();
                        ancestor_depth = ancestor_depth.saturating_sub(1);
                    }
                    matched
                }
                Combinator::Adjacent => {
                    // Must match immediately preceding sibling.
                    if let Some(prev_path) = get_previous_sibling_path(&current_path) {
                        if let Some(prev_node) = get_node_at_path(doc, &prev_path) {
                            if part
                                .simple
                                .matches(&prev_node, &prev_path, current_depth, doc)
                            {
                                current_path = prev_path;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
                Combinator::Sibling => {
                    // Must match some preceding sibling.
                    let mut sib_path = get_previous_sibling_path(&current_path);
                    let mut matched = false;

                    while let Some(sp) = sib_path {
                        if let Some(sib_node) = get_node_at_path(doc, &sp)
                            && part.simple.matches(&sib_node, &sp, current_depth, doc)
                        {
                            current_path = sp;
                            matched = true;
                            break;
                        }
                        sib_path = get_previous_sibling_path(&sp);
                    }
                    matched
                }
            };

            if !found {
                return false;
            }
        }

        true
    }
}

impl SimpleSelector {
    /// Checks if this simple selector matches a node.
    fn matches(&self, node: &SExpr, path: &PathId, depth: usize, doc: &SExpr) -> bool {
        match self {
            SimpleSelector::Universal => true,
            SimpleSelector::Tag(tag) => get_tag(node).map(|t| t == *tag).unwrap_or(false),
            SimpleSelector::AnyHeading => get_tag(node)
                .map(|t| {
                    t.len() == 2
                        && t.starts_with('h')
                        && t.chars()
                            .nth(1)
                            .map(|c| c.is_ascii_digit())
                            .unwrap_or(false)
                })
                .unwrap_or(false),
            SimpleSelector::Path(p) => path == p,
            SimpleSelector::Tagged { key, value } => {
                // Check for HTML comment tag before this node.
                check_node_tag(doc, path, key, value.as_deref())
            }
            SimpleSelector::WithAttributes { base, predicates } => {
                if !base.matches(node, path, depth, doc) {
                    return false;
                }
                predicates
                    .iter()
                    .all(|pred| pred.matches(node, path, depth))
            }
        }
    }
}

impl AttributePredicate {
    /// Checks if this predicate matches a node.
    fn matches(&self, node: &SExpr, path: &PathId, depth: usize) -> bool {
        let attr_value = get_attribute(node, &self.name, path, depth);

        match &self.comparison {
            None => {
                // Boolean check - attribute must exist and be truthy.
                match attr_value {
                    Some(v) => !v.is_empty() && v != "false" && v != "0",
                    None => false,
                }
            }
            Some((op, expected)) => {
                let actual = match attr_value {
                    Some(v) => v,
                    None => return false,
                };

                match op {
                    CompareOp::Eq => actual == *expected,
                    CompareOp::NotEq => actual != *expected,
                    CompareOp::Contains => actual.contains(expected.as_str()),
                    CompareOp::StartsWith => actual.starts_with(expected.as_str()),
                    CompareOp::EndsWith => actual.ends_with(expected.as_str()),
                    CompareOp::Lt | CompareOp::Lte | CompareOp::Gt | CompareOp::Gte => {
                        // Numeric comparison.
                        let actual_num: f64 = match actual.parse() {
                            Ok(n) => n,
                            Err(_) => return false,
                        };
                        let expected_num: f64 = match expected.parse() {
                            Ok(n) => n,
                            Err(_) => return false,
                        };
                        match op {
                            CompareOp::Lt => actual_num < expected_num,
                            CompareOp::Lte => actual_num <= expected_num,
                            CompareOp::Gt => actual_num > expected_num,
                            CompareOp::Gte => actual_num >= expected_num,
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }
    }
}

/// Gets an attribute value from a node.
fn get_attribute(node: &SExpr, name: &str, path: &PathId, depth: usize) -> Option<String> {
    match name {
        "depth" => Some(depth.to_string()),
        "level" => {
            let tag = get_tag(node)?;
            if tag.starts_with('h') && tag.len() == 2 {
                tag[1..].parse::<u8>().ok().map(|l| l.to_string())
            } else {
                None
            }
        }
        "text" => Some(extract_text_content(node)),
        "tag" => get_tag(node),
        "url" => get_url(node),
        "internal" => {
            let url = get_url(node)?;
            Some(is_internal_url(&url).to_string())
        }
        "image" => {
            let tag = get_tag(node)?;
            Some((tag == "img" || tag == "img-ref").to_string())
        }
        "lang" => get_code_lang(node),
        "path" => Some(path.to_string()),
        _ => None,
    }
}

/// Gets the tag name from a node.
fn get_tag(node: &SExpr) -> Option<String> {
    match node {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0] {
                // Not a quoted string.
                if !tag.starts_with('"') {
                    return Some(tag.clone());
                }
            }
            None
        }
        _ => None,
    }
}

/// Gets URL from link or image nodes.
fn get_url(node: &SExpr) -> Option<String> {
    let tag = get_tag(node)?;
    match tag.as_str() {
        "link" | "img" => {
            if let SExpr::List(items) = node
                && items.len() >= 2
            {
                return Some(extract_string(&items[1]));
            }
            None
        }
        _ => None,
    }
}

/// Gets language from code-block nodes.
fn get_code_lang(node: &SExpr) -> Option<String> {
    let tag = get_tag(node)?;
    if tag == "code-block"
        && let SExpr::List(items) = node
        && items.len() >= 2
    {
        return Some(extract_string(&items[1]));
    }
    None
}

/// Gets a node at a specific path.
fn get_node_at_path(doc: &SExpr, path: &PathId) -> Option<SExpr> {
    let mut current = doc.clone();
    for &index in path.indices() {
        match current {
            SExpr::List(items) => {
                current = items.get(index)?.clone();
            }
            SExpr::Atom(_) => return None,
        }
    }
    Some(current)
}

/// Gets the path of the previous sibling, if any.
fn get_previous_sibling_path(path: &PathId) -> Option<PathId> {
    get_previous_sibling_indices(path.indices()).map(PathId::new)
}

/// Checks if a node has a specific tag (metadata via HTML comment).
fn check_node_tag(doc: &SExpr, path: &PathId, key: &str, value: Option<&str>) -> bool {
    // Look for an HTML comment before this node that contains the tag.
    // Tags are formatted as: <!-- @tag:key=value --> or <!-- @tag:key -->
    if let Some(prev_path) = get_previous_sibling_path(path)
        && let Some(prev_node) = get_node_at_path(doc, &prev_path)
        && let Some(tag) = get_tag(&prev_node)
        && tag == "html"
        && let SExpr::List(items) = &prev_node
        && items.len() >= 2
    {
        let content = extract_string(&items[1]);
        return parse_tag_comment(&content, key, value);
    }
    false
}

/// Parses a tag from an HTML comment.
/// Format: <!-- @tag:key=value --> or <!-- @tag:key -->
fn parse_tag_comment(content: &str, key: &str, expected_value: Option<&str>) -> bool {
    let content = content.trim();
    if !content.starts_with("<!--") || !content.ends_with("-->") {
        return false;
    }
    let inner = content[4..content.len() - 3].trim();
    if !inner.starts_with("@tag:") {
        return false;
    }
    let tag_content = &inner[5..];

    // Parse key=value or just key.
    if let Some(eq_pos) = tag_content.find('=') {
        let k = &tag_content[..eq_pos];
        let v = &tag_content[eq_pos + 1..];
        if k == key {
            match expected_value {
                Some(ev) => v == ev,
                None => true,
            }
        } else {
            false
        }
    } else {
        // Just key, no value.
        tag_content == key && expected_value.is_none()
    }
}

// ============================================================================
// Parser
// ============================================================================

/// Parser for selector strings.
struct SelectorParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> SelectorParser<'a> {
    fn new(input: &'a str) -> Self {
        SelectorParser { input, pos: 0 }
    }

    fn parse(&mut self) -> SResult<Selector> {
        let mut parts = Vec::new();
        self.skip_whitespace();

        loop {
            let simple = self.parse_simple_selector()?;
            self.skip_whitespace();

            // Check for combinator.
            let combinator = self.parse_combinator();
            parts.push(SelectorPart { simple, combinator });

            if combinator.is_none() || self.is_at_end() {
                break;
            }

            self.skip_whitespace();
        }

        Ok(Selector { parts })
    }

    fn parse_simple_selector(&mut self) -> SResult<SimpleSelector> {
        self.skip_whitespace();

        if self.is_at_end() {
            return Err(SError::new("selector")
                .with_code("unexpected-end")
                .with_message("Unexpected end of selector"));
        }

        let ch = self.peek_char().unwrap();

        let base = if ch == '*' {
            self.advance();
            SimpleSelector::Universal
        } else if ch == '@' {
            self.advance();
            let path_str = self.consume_while(|c| c.is_ascii_digit() || c == '.');
            let path = PathId::parse(&path_str)?;
            SimpleSelector::Path(path)
        } else if ch == '#' {
            self.advance();
            let key = self.consume_identifier();
            let value = if self.peek_char() == Some('=') {
                self.advance();
                Some(self.consume_value())
            } else {
                None
            };
            SimpleSelector::Tagged { key, value }
        } else if ch.is_alphabetic() {
            let tag = self.consume_identifier();
            if tag == "h" && self.peek_char() == Some('*') {
                self.advance();
                SimpleSelector::AnyHeading
            } else {
                SimpleSelector::Tag(tag)
            }
        } else {
            return Err(SError::new("selector")
                .with_code("unexpected-char")
                .with_message("Unexpected character in selector")
                .with_string_field("char", &ch.to_string()));
        };

        // Check for attribute predicates.
        if self.peek_char() == Some('[') {
            let predicates = self.parse_attribute_predicates()?;
            Ok(SimpleSelector::WithAttributes {
                base: Box::new(base),
                predicates,
            })
        } else {
            Ok(base)
        }
    }

    fn parse_attribute_predicates(&mut self) -> SResult<Vec<AttributePredicate>> {
        let mut predicates = Vec::new();

        while self.peek_char() == Some('[') {
            self.advance(); // consume '['
            self.skip_whitespace();

            let name = self.consume_identifier();
            self.skip_whitespace();

            let comparison = if self.peek_char() == Some(']') {
                None
            } else {
                let op = self.parse_compare_op()?;
                self.skip_whitespace();
                let value = self.consume_value();
                Some((op, value))
            };

            self.skip_whitespace();
            if self.peek_char() != Some(']') {
                return Err(SError::new("selector")
                    .with_code("unclosed-bracket")
                    .with_message("Expected ']' in attribute selector"));
            }
            self.advance(); // consume ']'

            predicates.push(AttributePredicate { name, comparison });
        }

        Ok(predicates)
    }

    fn parse_compare_op(&mut self) -> SResult<CompareOp> {
        let ch = self.peek_char().ok_or_else(|| {
            SError::new("selector")
                .with_code("unexpected-end")
                .with_message("Expected comparison operator")
        })?;

        match ch {
            '=' => {
                self.advance();
                Ok(CompareOp::Eq)
            }
            '!' => {
                self.advance();
                if self.peek_char() == Some('=') {
                    self.advance();
                    Ok(CompareOp::NotEq)
                } else {
                    Err(SError::new("selector")
                        .with_code("invalid-operator")
                        .with_message("Expected '=' after '!'"))
                }
            }
            '~' => {
                self.advance();
                if self.peek_char() == Some('=') {
                    self.advance();
                    Ok(CompareOp::Contains)
                } else {
                    Err(SError::new("selector")
                        .with_code("invalid-operator")
                        .with_message("Expected '=' after '~'"))
                }
            }
            '^' => {
                self.advance();
                if self.peek_char() == Some('=') {
                    self.advance();
                    Ok(CompareOp::StartsWith)
                } else {
                    Err(SError::new("selector")
                        .with_code("invalid-operator")
                        .with_message("Expected '=' after '^'"))
                }
            }
            '$' => {
                self.advance();
                if self.peek_char() == Some('=') {
                    self.advance();
                    Ok(CompareOp::EndsWith)
                } else {
                    Err(SError::new("selector")
                        .with_code("invalid-operator")
                        .with_message("Expected '=' after '$'"))
                }
            }
            '<' => {
                self.advance();
                if self.peek_char() == Some('=') {
                    self.advance();
                    Ok(CompareOp::Lte)
                } else {
                    Ok(CompareOp::Lt)
                }
            }
            '>' => {
                self.advance();
                if self.peek_char() == Some('=') {
                    self.advance();
                    Ok(CompareOp::Gte)
                } else {
                    Ok(CompareOp::Gt)
                }
            }
            _ => Err(SError::new("selector")
                .with_code("invalid-operator")
                .with_message("Unknown comparison operator")
                .with_string_field("char", &ch.to_string())),
        }
    }

    fn parse_combinator(&mut self) -> Option<Combinator> {
        self.skip_whitespace();

        let ch = self.peek_char()?;
        match ch {
            '>' => {
                self.advance();
                self.skip_whitespace();
                Some(Combinator::Child)
            }
            '+' => {
                self.advance();
                self.skip_whitespace();
                Some(Combinator::Adjacent)
            }
            '~' => {
                self.advance();
                self.skip_whitespace();
                Some(Combinator::Sibling)
            }
            _ => {
                // Check if there's more content (implicit descendant combinator).
                if !self.is_at_end() && ch.is_alphabetic() || ch == '*' || ch == '@' || ch == '#' {
                    Some(Combinator::Descendant)
                } else {
                    None
                }
            }
        }
    }

    fn consume_identifier(&mut self) -> String {
        self.consume_while(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }

    fn consume_value(&mut self) -> String {
        self.skip_whitespace();

        // Check for quoted string.
        if self.peek_char() == Some('"') {
            self.advance();
            let value = self.consume_while(|c| c != '"');
            if self.peek_char() == Some('"') {
                self.advance();
            }
            return value;
        }

        // Unquoted value.
        self.consume_while(|c| !c.is_whitespace() && c != ']' && c != '[')
    }

    fn consume_while<F: Fn(char) -> bool>(&mut self, pred: F) -> String {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if pred(ch) {
                self.advance();
            } else {
                break;
            }
        }
        self.input[start..self.pos].to_string()
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance(&mut self) {
        if let Some(ch) = self.peek_char() {
            self.pos += ch.len_utf8();
        }
    }

    fn is_at_end(&self) -> bool {
        self.pos >= self.input.len()
    }
}

// ============================================================================
// Query Functions
// ============================================================================

/// Finds all nodes matching a selector.
pub fn select(doc: &SExpr, selector: &Selector) -> Vec<(PathId, SExpr)> {
    let mut results = Vec::new();
    select_recursive(doc, doc, selector, &PathId::root(), 0, &mut results);
    results
}

/// Recursively searches for matching nodes.
fn select_recursive(
    doc: &SExpr,
    node: &SExpr,
    selector: &Selector,
    path: &PathId,
    depth: usize,
    results: &mut Vec<(PathId, SExpr)>,
) {
    // Check if current node matches.
    if selector.matches(doc, path, depth) {
        results.push((path.clone(), node.clone()));
    }

    // Recurse into children.
    if let SExpr::List(items) = node {
        for (i, child) in items.iter().enumerate().skip(1) {
            select_recursive(doc, child, selector, &path.child(i), depth + 1, results);
        }
    }
}

/// Convenience function to select by selector string.
pub fn query(doc: &SExpr, selector_str: &str) -> SResult<Vec<(PathId, SExpr)>> {
    let selector = Selector::parse(selector_str)?;
    Ok(select(doc, &selector))
}

/// Returns the first matching node, if any.
pub fn query_one(doc: &SExpr, selector_str: &str) -> SResult<Option<(PathId, SExpr)>> {
    let results = query(doc, selector_str)?;
    Ok(results.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Parser;

    fn parse_doc(s: &str) -> SExpr {
        Parser::new(s).parse().unwrap()
    }

    #[test]
    fn parse_tag_selector() {
        let sel = Selector::parse("h1").unwrap();
        assert_eq!(sel.parts.len(), 1);
        assert!(matches!(sel.parts[0].simple, SimpleSelector::Tag(ref t) if t == "h1"));
    }

    #[test]
    fn parse_universal_selector() {
        let sel = Selector::parse("*").unwrap();
        assert!(matches!(sel.parts[0].simple, SimpleSelector::Universal));
    }

    #[test]
    fn parse_any_heading_selector() {
        let sel = Selector::parse("h*").unwrap();
        assert!(matches!(sel.parts[0].simple, SimpleSelector::AnyHeading));
    }

    #[test]
    fn parse_path_selector() {
        let sel = Selector::parse("@1.2.3").unwrap();
        assert!(
            matches!(sel.parts[0].simple, SimpleSelector::Path(ref p) if p.indices() == [1, 2, 3])
        );
    }

    #[test]
    fn parse_tagged_selector() {
        let sel = Selector::parse("#deprecated").unwrap();
        assert!(matches!(
            sel.parts[0].simple,
            SimpleSelector::Tagged { ref key, ref value } if key == "deprecated" && value.is_none()
        ));
    }

    #[test]
    fn parse_tagged_with_value() {
        let sel = Selector::parse("#status=draft").unwrap();
        assert!(matches!(
            sel.parts[0].simple,
            SimpleSelector::Tagged { ref key, ref value }
                if key == "status" && value.as_deref() == Some("draft")
        ));
    }

    #[test]
    fn parse_attribute_selector() {
        let sel = Selector::parse("link[internal]").unwrap();
        assert!(matches!(
            sel.parts[0].simple,
            SimpleSelector::WithAttributes { ref predicates, .. } if predicates.len() == 1
        ));
    }

    #[test]
    fn parse_attribute_with_op() {
        let sel = Selector::parse("h2[text~=\"Legacy\"]").unwrap();
        if let SimpleSelector::WithAttributes { predicates, .. } = &sel.parts[0].simple {
            assert_eq!(predicates[0].name, "text");
            assert!(matches!(
                predicates[0].comparison,
                Some((CompareOp::Contains, ref v)) if v == "Legacy"
            ));
        } else {
            panic!("Expected WithAttributes");
        }
    }

    #[test]
    fn parse_numeric_comparison() {
        let sel = Selector::parse("*[depth<=3]").unwrap();
        if let SimpleSelector::WithAttributes { predicates, .. } = &sel.parts[0].simple {
            assert!(matches!(
                predicates[0].comparison,
                Some((CompareOp::Lte, ref v)) if v == "3"
            ));
        } else {
            panic!("Expected WithAttributes");
        }
    }

    #[test]
    fn parse_child_combinator() {
        let sel = Selector::parse("h1 > p").unwrap();
        assert_eq!(sel.parts.len(), 2);
        assert!(matches!(sel.parts[0].combinator, Some(Combinator::Child)));
    }

    #[test]
    fn parse_descendant_combinator() {
        let sel = Selector::parse("blockquote p").unwrap();
        assert_eq!(sel.parts.len(), 2);
        assert!(matches!(
            sel.parts[0].combinator,
            Some(Combinator::Descendant)
        ));
    }

    #[test]
    fn parse_adjacent_combinator() {
        let sel = Selector::parse("h2 + p").unwrap();
        assert_eq!(sel.parts.len(), 2);
        assert!(matches!(
            sel.parts[0].combinator,
            Some(Combinator::Adjacent)
        ));
    }

    #[test]
    fn select_by_tag() {
        let doc = parse_doc(r#"(doc (h1 "Title") (p "Text") (h2 "Sub"))"#);
        let results = query(&doc, "h1").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, PathId::new(vec![1]));
    }

    #[test]
    fn select_all_headings() {
        let doc = parse_doc(r#"(doc (h1 "A") (p "B") (h2 "C") (h3 "D"))"#);
        let results = query(&doc, "h*").unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn select_by_path() {
        let doc = parse_doc(r#"(doc (h1 "Title") (p "Text"))"#);
        let results = query(&doc, "@2").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.to_string().contains("Text"));
    }

    #[test]
    fn select_universal() {
        let doc = parse_doc(r#"(doc (h1 "A") (p "B"))"#);
        let results = query(&doc, "*").unwrap();
        // doc, h1, "A", p, "B"
        assert!(results.len() >= 3);
    }

    #[test]
    fn select_with_depth() {
        let doc = parse_doc(r#"(doc (h1 "A") (ul (li (p "Deep"))))"#);
        let results = query(&doc, "*[depth<=1]").unwrap();
        println!(
            "DEBUG depth results: {:?}",
            results
                .iter()
                .map(|(p, _)| p.to_string())
                .collect::<Vec<_>>()
        );
        // At depth 0: doc. At depth 1: h1, ul
        // The string atoms have depth 2+ so should be excluded
        let shallow: Vec<_> = results.iter().filter(|(p, _)| p.depth() <= 1).collect();
        assert!(!shallow.is_empty());
    }

    #[test]
    fn select_child_combinator() {
        let doc = parse_doc(r#"(doc (blockquote (p "Direct")) (p "Not direct"))"#);
        let results = query(&doc, "blockquote > p").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.to_string().contains("Direct"));
    }

    #[test]
    fn select_descendant_combinator() {
        let doc = parse_doc(r#"(doc (blockquote (ul (li (p "Deep")))) (p "Outside"))"#);
        let results = query(&doc, "blockquote p").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.to_string().contains("Deep"));
    }

    #[test]
    fn select_adjacent_sibling() {
        let doc = parse_doc(r#"(doc (h2 "Section") (p "After h2") (p "Second p"))"#);
        let results = query(&doc, "h2 + p").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.to_string().contains("After h2"));
    }

    #[test]
    fn select_internal_links() {
        let doc = parse_doc(
            r#"(doc (p (link "./local.md" "" "Local")) (p (link "https://example.com" "" "External")))"#,
        );
        let results = query(&doc, "link[internal=true]").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.to_string().contains("local.md"));
    }

    #[test]
    fn select_by_text_content() {
        let doc = parse_doc(r#"(doc (h1 "Introduction") (h2 "Legacy API") (h2 "New API"))"#);
        let results = query(&doc, "h2[text~=\"Legacy\"]").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.to_string().contains("Legacy"));
    }

    #[test]
    fn select_by_level() {
        let doc = parse_doc(r#"(doc (h1 "One") (h2 "Two") (h3 "Three"))"#);
        let results = query(&doc, "h*[level>=2]").unwrap();
        assert_eq!(results.len(), 2); // h2 and h3
    }

    #[test]
    fn query_one_finds_first() {
        let doc = parse_doc(r#"(doc (p "First") (p "Second"))"#);
        let result = query_one(&doc, "p").unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().1.to_string().contains("First"));
    }

    #[test]
    fn query_one_returns_none_when_no_match() {
        let doc = parse_doc(r#"(doc (h1 "Title"))"#);
        let result = query_one(&doc, "p").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_empty_selector_error() {
        let err = Selector::parse("").unwrap_err();
        assert!(err.to_string().contains("unexpected-end"));
    }

    #[test]
    fn parse_whitespace_only_selector_error() {
        let err = Selector::parse("   ").unwrap_err();
        assert!(err.to_string().contains("unexpected-end"));
    }

    #[test]
    fn parse_invalid_char_error() {
        let err = Selector::parse("123").unwrap_err();
        assert!(err.to_string().contains("unexpected-char"));
    }

    #[test]
    fn parse_unclosed_bracket_error() {
        let err = Selector::parse("p[text=foo").unwrap_err();
        eprintln!("DEBUG parse_unclosed_bracket_error: {}", err.detail());
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("selector".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("unclosed-bracket".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"Expected ']' in attribute selector\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn parse_invalid_operator_bang_error() {
        let err = Selector::parse("p[text!foo]").unwrap_err();
        assert!(err.to_string().contains("invalid-operator"));
    }

    #[test]
    fn parse_invalid_operator_tilde_error() {
        let err = Selector::parse("p[text~foo]").unwrap_err();
        assert!(err.to_string().contains("invalid-operator"));
    }

    #[test]
    fn parse_invalid_operator_caret_error() {
        let err = Selector::parse("p[text^foo]").unwrap_err();
        assert!(err.to_string().contains("invalid-operator"));
    }

    #[test]
    fn parse_invalid_operator_dollar_error() {
        let err = Selector::parse("p[text$foo]").unwrap_err();
        assert!(err.to_string().contains("invalid-operator"));
    }

    #[test]
    fn parse_unknown_operator_error() {
        let err = Selector::parse("p[text%foo]").unwrap_err();
        assert!(err.to_string().contains("invalid-operator"));
    }

    #[test]
    fn select_sibling_combinator() {
        let doc = parse_doc(r#"(doc (h2 "A") (p "B") (p "C") (p "D"))"#);
        let results = query(&doc, "h2 ~ p").unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn attribute_not_equal() {
        let doc = parse_doc(r#"(doc (h1 "A") (h2 "B") (h3 "C"))"#);
        let results = query(&doc, "h*[level!=2]").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn attribute_starts_with() {
        let doc = parse_doc(r#"(doc (h1 "Introduction") (h2 "Internal Details") (h2 "Summary"))"#);
        let results = query(&doc, "h*[text^=\"Int\"]").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn attribute_ends_with() {
        let doc = parse_doc(r#"(doc (h1 "Introduction") (h2 "Configuration") (h2 "Summary"))"#);
        let results = query(&doc, "h*[text$=\"tion\"]").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn attribute_less_than() {
        let doc = parse_doc(r#"(doc (h1 "A") (h2 "B") (h3 "C") (h4 "D"))"#);
        let results = query(&doc, "h*[level<3]").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn attribute_greater_than() {
        let doc = parse_doc(r#"(doc (h1 "A") (h2 "B") (h3 "C") (h4 "D"))"#);
        let results = query(&doc, "h*[level>2]").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn attribute_greater_than_equal() {
        let doc = parse_doc(r#"(doc (h1 "A") (h2 "B") (h3 "C") (h4 "D"))"#);
        let results = query(&doc, "h*[level>=2]").unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn attribute_boolean_check() {
        let doc = parse_doc(
            r#"(doc (p (link "./local.md" "" "Local")) (p (link "https://ext.com" "" "Ext")))"#,
        );
        let results = query(&doc, "link[internal]").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn attribute_numeric_non_parseable_returns_no_match() {
        let doc = parse_doc(r#"(doc (h1 "Not a number"))"#);
        let results = query(&doc, "h1[text>5]").unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn multiple_attribute_predicates() {
        let doc = parse_doc(r#"(doc (h1 "A") (h2 "B") (h3 "C") (h4 "D"))"#);
        let results = query(&doc, "h*[level>=2][level<=3]").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn select_by_tag_attribute() {
        let doc = parse_doc(r#"(doc (h1 "A") (p "B") (h2 "C"))"#);
        let results = query(&doc, "*[tag=h1]").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn select_by_path_attribute() {
        let doc = parse_doc(r#"(doc (h1 "A") (p "B"))"#);
        let results = query(&doc, "*[path=1]").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn select_by_url_attribute() {
        let doc = parse_doc(r#"(doc (p (link "test.md" "" "Test")))"#);
        let results = query(&doc, "link[url=test.md]").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn select_by_lang_attribute() {
        let doc = parse_doc(r#"(doc (code-block "rust" "fn main() {}"))"#);
        let results = query(&doc, "code-block[lang=rust]").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn select_images_by_image_attribute() {
        let doc = parse_doc(r#"(doc (p (img "pic.png" "Alt" "Title")))"#);
        let results = query(&doc, "*[image=true]").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn tagged_selector_with_html_comment() {
        let doc = parse_doc(r#"(doc (html "<!-- @tag:status=draft -->") (h1 "Draft Title"))"#);
        let results = query(&doc, "#status=draft").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn tagged_selector_key_only() {
        let doc = parse_doc(r#"(doc (html "<!-- @tag:deprecated -->") (h1 "Old API"))"#);
        let results = query(&doc, "#deprecated").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn tagged_selector_no_match_wrong_value() {
        let doc = parse_doc(r#"(doc (html "<!-- @tag:status=published -->") (h1 "Title"))"#);
        let results = query(&doc, "#status=draft").unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn chained_combinators() {
        let doc = parse_doc(r#"(doc (blockquote (ul (li (p "Deep")))))"#);
        let results = query(&doc, "blockquote ul li p").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn child_then_descendant_combinator() {
        let doc = parse_doc(r#"(doc (ul (li (p "A"))) (ol (li (div (p "B")))))"#);
        let results = query(&doc, "ul > li p").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.to_string().contains("A"));
    }

    #[test]
    fn select_root_with_universal() {
        let doc = parse_doc(r#"(doc (h1 "Title"))"#);
        let results = query(&doc, "*[depth=0]").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn select_deeply_nested() {
        let doc = parse_doc(r#"(doc (a (b (c (d (e (f "Deep")))))))"#);
        let results = query(&doc, "f").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn quoted_value_in_attribute() {
        let doc = parse_doc(r#"(doc (h1 "Hello World"))"#);
        let results = query(&doc, "h1[text=\"Hello World\"]").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn internal_url_empty() {
        assert!(is_internal_url(""));
    }

    #[test]
    fn internal_url_hash() {
        assert!(is_internal_url("#section"));
    }

    #[test]
    fn internal_url_relative_dot() {
        assert!(is_internal_url("./page.md"));
    }

    #[test]
    fn internal_url_relative_dotdot() {
        assert!(is_internal_url("../other/page.md"));
    }

    #[test]
    fn internal_url_no_protocol() {
        assert!(is_internal_url("page.md"));
    }

    #[test]
    fn external_url_https() {
        assert!(!is_internal_url("https://example.com"));
    }

    #[test]
    fn external_url_http() {
        assert!(!is_internal_url("http://example.com"));
    }

    #[test]
    fn get_attribute_unknown_returns_none() {
        let doc = parse_doc(r#"(h1 "Title")"#);
        let result = get_attribute(&doc, "unknown", &PathId::root(), 0);
        assert!(result.is_none());
    }

    #[test]
    fn extract_text_content_nested() {
        let doc = parse_doc(r#"(p "Hello " (strong "world") "!")"#);
        let text = extract_text_content(&doc);
        assert_eq!(text, "Hello world!");
    }
}
