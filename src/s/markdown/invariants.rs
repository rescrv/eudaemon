//! Structural invariant validation for markdown documents.
//!
//! This module allows defining and checking structural rules for documents,
//! enabling "unit tests for documentation structure." Invariants can verify
//! that documents follow consistent patterns and catch regressions.

use crate::s::expr::SExpr;
use crate::s::nodeid::PathId;
use crate::s::selector::{Selector, query};

/// Result of validating an invariant.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the invariant passed.
    pub passed: bool,
    /// Human-readable description of the invariant.
    pub description: String,
    /// List of violations (empty if passed).
    pub violations: Vec<Violation>,
}

impl ValidationResult {
    /// Creates a passing validation result.
    pub fn pass(description: &str) -> Self {
        ValidationResult {
            passed: true,
            description: description.to_string(),
            violations: Vec::new(),
        }
    }

    /// Creates a failing validation result with violations.
    pub fn fail(description: &str, violations: Vec<Violation>) -> Self {
        ValidationResult {
            passed: false,
            description: description.to_string(),
            violations,
        }
    }
}

/// A specific violation of an invariant.
#[derive(Debug, Clone)]
pub struct Violation {
    /// Path to the violating node.
    pub path: PathId,
    /// Description of the violation.
    pub message: String,
    /// Optional context (e.g., the node's text content).
    pub context: Option<String>,
}

impl Violation {
    /// Creates a new violation.
    pub fn new(path: PathId, message: &str) -> Self {
        Violation {
            path,
            message: message.to_string(),
            context: None,
        }
    }

    /// Creates a violation with context.
    pub fn with_context(path: PathId, message: &str, context: &str) -> Self {
        Violation {
            path,
            message: message.to_string(),
            context: Some(context.to_string()),
        }
    }
}

/// A predicate that can be checked against document structure.
#[derive(Debug, Clone)]
pub enum Invariant {
    /// Every node matching `selector` must have a child matching `child_selector`.
    HasChild {
        selector: String,
        child_selector: String,
    },
    /// Every node matching `selector` must have a sibling matching `sibling_selector`.
    HasSibling {
        selector: String,
        sibling_selector: String,
    },
    /// Nodes matching `selector` must not exist (useful for banning patterns).
    MustNotExist { selector: String },
    /// At least one node matching `selector` must exist.
    MustExist { selector: String },
    /// Nodes matching `selector` must be at depth <= max_depth.
    MaxDepth { selector: String, max_depth: usize },
    /// Nodes matching `selector` must have an associated tag.
    RequiresTag { selector: String, tag_key: String },
    /// Heading levels must not skip (e.g., h1 -> h3 without h2).
    NoHeadingSkips,
    /// Custom predicate using a closure.
    Custom {
        description: String,
        predicate: fn(&SExpr, &PathId) -> bool,
    },
}

/// Validates a document against an invariant.
pub fn assert_invariant(doc: &SExpr, invariant: &Invariant) -> ValidationResult {
    match invariant {
        Invariant::HasChild {
            selector,
            child_selector,
        } => validate_has_child(doc, selector, child_selector),

        Invariant::HasSibling {
            selector,
            sibling_selector,
        } => validate_has_sibling(doc, selector, sibling_selector),

        Invariant::MustNotExist { selector } => validate_must_not_exist(doc, selector),

        Invariant::MustExist { selector } => validate_must_exist(doc, selector),

        Invariant::MaxDepth {
            selector,
            max_depth,
        } => validate_max_depth(doc, selector, *max_depth),

        Invariant::RequiresTag { selector, tag_key } => {
            validate_requires_tag(doc, selector, tag_key)
        }

        Invariant::NoHeadingSkips => validate_no_heading_skips(doc),

        Invariant::Custom {
            description,
            predicate,
        } => validate_custom(doc, description, *predicate),
    }
}

/// Validates that every node matching selector has a child matching child_selector.
fn validate_has_child(doc: &SExpr, selector: &str, child_selector: &str) -> ValidationResult {
    let desc = format!(
        "Every '{}' must have a '{}' child",
        selector, child_selector
    );

    let matches = match query(doc, selector) {
        Ok(m) => m,
        Err(e) => {
            return ValidationResult::fail(
                &desc,
                vec![Violation::new(
                    PathId::root(),
                    &format!("Invalid selector: {}", e),
                )],
            );
        }
    };

    let child_sel = match Selector::parse(child_selector) {
        Ok(s) => s,
        Err(e) => {
            return ValidationResult::fail(
                &desc,
                vec![Violation::new(
                    PathId::root(),
                    &format!("Invalid child selector: {}", e),
                )],
            );
        }
    };

    let mut violations = Vec::new();

    for (path, node) in matches {
        // Check if any child matches child_selector.
        let has_matching_child = if let SExpr::List(items) = &node {
            items.iter().enumerate().skip(1).any(|(i, child)| {
                let child_path = path.child(i);
                child_sel.matches(doc, &child_path, path.depth() + 1)
                    || has_matching_descendant(
                        doc,
                        child,
                        &child_path,
                        &child_sel,
                        path.depth() + 1,
                    )
            })
        } else {
            false
        };

        if !has_matching_child {
            violations.push(Violation::new(
                path.clone(),
                &format!("Missing '{}' child", child_selector),
            ));
        }
    }

    if violations.is_empty() {
        ValidationResult::pass(&desc)
    } else {
        ValidationResult::fail(&desc, violations)
    }
}

/// Checks if any descendant matches the selector.
fn has_matching_descendant(
    doc: &SExpr,
    node: &SExpr,
    path: &PathId,
    selector: &Selector,
    depth: usize,
) -> bool {
    if selector.matches(doc, path, depth) {
        return true;
    }

    if let SExpr::List(items) = node {
        for (i, child) in items.iter().enumerate().skip(1) {
            if has_matching_descendant(doc, child, &path.child(i), selector, depth + 1) {
                return true;
            }
        }
    }

    false
}

/// Validates that every node matching selector has a sibling matching sibling_selector.
fn validate_has_sibling(doc: &SExpr, selector: &str, sibling_selector: &str) -> ValidationResult {
    let desc = format!(
        "Every '{}' must have a '{}' sibling",
        selector, sibling_selector
    );

    let matches = match query(doc, selector) {
        Ok(m) => m,
        Err(e) => {
            return ValidationResult::fail(
                &desc,
                vec![Violation::new(
                    PathId::root(),
                    &format!("Invalid selector: {}", e),
                )],
            );
        }
    };

    let sibling_sel = match Selector::parse(sibling_selector) {
        Ok(s) => s,
        Err(e) => {
            return ValidationResult::fail(
                &desc,
                vec![Violation::new(
                    PathId::root(),
                    &format!("Invalid sibling selector: {}", e),
                )],
            );
        }
    };

    let mut violations = Vec::new();

    for (path, _node) in matches {
        let siblings = get_siblings_for_path(doc, &path);
        let has_matching_sibling = siblings.iter().any(|(sib_path, _)| {
            sib_path != &path && sibling_sel.matches(doc, sib_path, path.depth())
        });

        if !has_matching_sibling {
            violations.push(Violation::new(
                path.clone(),
                &format!("Missing '{}' sibling", sibling_selector),
            ));
        }
    }

    if violations.is_empty() {
        ValidationResult::pass(&desc)
    } else {
        ValidationResult::fail(&desc, violations)
    }
}

/// Gets all siblings for a path.
fn get_siblings_for_path(doc: &SExpr, path: &PathId) -> Vec<(PathId, SExpr)> {
    let parent_path = match path.parent() {
        Some(p) => p,
        None => return vec![],
    };

    let parent = match crate::s::nodeid::get_by_path(doc, &parent_path) {
        Some(p) => p,
        None => return vec![],
    };

    if let SExpr::List(items) = parent {
        items
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, child)| (parent_path.child(i), child.clone()))
            .collect()
    } else {
        vec![]
    }
}

/// Validates that no nodes match the selector.
fn validate_must_not_exist(doc: &SExpr, selector: &str) -> ValidationResult {
    let desc = format!("No nodes should match '{}'", selector);

    let matches = match query(doc, selector) {
        Ok(m) => m,
        Err(e) => {
            return ValidationResult::fail(
                &desc,
                vec![Violation::new(
                    PathId::root(),
                    &format!("Invalid selector: {}", e),
                )],
            );
        }
    };

    if matches.is_empty() {
        ValidationResult::pass(&desc)
    } else {
        let violations: Vec<Violation> = matches
            .into_iter()
            .map(|(path, _)| {
                Violation::new(path, &format!("Unexpected node matching '{}'", selector))
            })
            .collect();
        ValidationResult::fail(&desc, violations)
    }
}

/// Validates that at least one node matches the selector.
fn validate_must_exist(doc: &SExpr, selector: &str) -> ValidationResult {
    let desc = format!("At least one node should match '{}'", selector);

    let matches = match query(doc, selector) {
        Ok(m) => m,
        Err(e) => {
            return ValidationResult::fail(
                &desc,
                vec![Violation::new(
                    PathId::root(),
                    &format!("Invalid selector: {}", e),
                )],
            );
        }
    };

    if matches.is_empty() {
        ValidationResult::fail(
            &desc,
            vec![Violation::new(
                PathId::root(),
                &format!("No nodes found matching '{}'", selector),
            )],
        )
    } else {
        ValidationResult::pass(&desc)
    }
}

/// Validates that nodes matching selector are at depth <= max_depth.
fn validate_max_depth(doc: &SExpr, selector: &str, max_depth: usize) -> ValidationResult {
    let desc = format!("'{}' nodes must be at depth <= {}", selector, max_depth);

    let matches = match query(doc, selector) {
        Ok(m) => m,
        Err(e) => {
            return ValidationResult::fail(
                &desc,
                vec![Violation::new(
                    PathId::root(),
                    &format!("Invalid selector: {}", e),
                )],
            );
        }
    };

    let violations: Vec<Violation> = matches
        .into_iter()
        .filter(|(path, _)| path.depth() > max_depth)
        .map(|(path, _)| {
            Violation::new(
                path.clone(),
                &format!(
                    "Node at depth {} exceeds max depth {}",
                    path.depth(),
                    max_depth
                ),
            )
        })
        .collect();

    if violations.is_empty() {
        ValidationResult::pass(&desc)
    } else {
        ValidationResult::fail(&desc, violations)
    }
}

/// Validates that nodes matching selector have an associated tag.
fn validate_requires_tag(doc: &SExpr, selector: &str, tag_key: &str) -> ValidationResult {
    let desc = format!("'{}' nodes must have '{}' tag", selector, tag_key);

    let matches = match query(doc, selector) {
        Ok(m) => m,
        Err(e) => {
            return ValidationResult::fail(
                &desc,
                vec![Violation::new(
                    PathId::root(),
                    &format!("Invalid selector: {}", e),
                )],
            );
        }
    };

    let tagged = crate::s::markdown::curation::get_tagged_nodes(doc, tag_key);
    let tagged_paths: std::collections::HashSet<_> = tagged.into_iter().map(|t| t.path).collect();

    let violations: Vec<Violation> = matches
        .into_iter()
        .filter(|(path, _)| !tagged_paths.contains(path))
        .map(|(path, _)| Violation::new(path, &format!("Missing '{}' tag", tag_key)))
        .collect();

    if violations.is_empty() {
        ValidationResult::pass(&desc)
    } else {
        ValidationResult::fail(&desc, violations)
    }
}

/// Validates that heading levels don't skip (e.g., h1 -> h3 without h2).
fn validate_no_heading_skips(doc: &SExpr) -> ValidationResult {
    let desc = "Heading levels should not skip".to_string();

    let mut violations = Vec::new();
    let mut last_level: Option<u8> = None;

    check_heading_skips_recursive(doc, &PathId::root(), &mut last_level, &mut violations);

    if violations.is_empty() {
        ValidationResult::pass(&desc)
    } else {
        ValidationResult::fail(&desc, violations)
    }
}

/// Recursively checks for heading level skips.
fn check_heading_skips_recursive(
    expr: &SExpr,
    path: &PathId,
    last_level: &mut Option<u8>,
    violations: &mut Vec<Violation>,
) {
    if let SExpr::List(items) = expr
        && !items.is_empty()
    {
        if let SExpr::Atom(tag) = &items[0]
            && tag.len() == 2
            && tag.starts_with('h')
            && let Ok(level) = tag[1..].parse::<u8>()
        {
            if let Some(prev) = *last_level {
                // Allow going deeper by 1, or going back to any shallower level.
                if level > prev + 1 {
                    violations.push(Violation::new(
                        path.clone(),
                        &format!(
                            "Heading level skipped from h{} to h{} (missing h{})",
                            prev,
                            level,
                            prev + 1
                        ),
                    ));
                }
            }
            *last_level = Some(level);
        }

        // Recurse into children.
        for (i, child) in items.iter().enumerate().skip(1) {
            check_heading_skips_recursive(child, &path.child(i), last_level, violations);
        }
    }
}

/// Validates using a custom predicate function.
fn validate_custom(
    doc: &SExpr,
    description: &str,
    predicate: fn(&SExpr, &PathId) -> bool,
) -> ValidationResult {
    let mut violations = Vec::new();
    check_custom_recursive(doc, &PathId::root(), predicate, &mut violations);

    if violations.is_empty() {
        ValidationResult::pass(description)
    } else {
        ValidationResult::fail(description, violations)
    }
}

/// Recursively applies custom predicate.
fn check_custom_recursive(
    expr: &SExpr,
    path: &PathId,
    predicate: fn(&SExpr, &PathId) -> bool,
    violations: &mut Vec<Violation>,
) {
    if !predicate(expr, path) {
        violations.push(Violation::new(path.clone(), "Custom predicate failed"));
    }

    if let SExpr::List(items) = expr {
        for (i, child) in items.iter().enumerate().skip(1) {
            check_custom_recursive(child, &path.child(i), predicate, violations);
        }
    }
}

/// Validates multiple invariants and returns all results.
pub fn validate_all(doc: &SExpr, invariants: &[Invariant]) -> Vec<ValidationResult> {
    invariants
        .iter()
        .map(|inv| assert_invariant(doc, inv))
        .collect()
}

/// Returns true if all invariants pass.
pub fn all_pass(doc: &SExpr, invariants: &[Invariant]) -> bool {
    invariants
        .iter()
        .all(|inv| assert_invariant(doc, inv).passed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s::expr::Parser;

    fn parse(s: &str) -> SExpr {
        Parser::new(s).parse().unwrap()
    }

    #[test]
    fn invariant_must_exist_pass() {
        let doc = parse(r#"(doc (h1 "Title") (p "Content"))"#);
        let inv = Invariant::MustExist {
            selector: "h1".to_string(),
        };
        let result = assert_invariant(&doc, &inv);
        println!("DEBUG must_exist result: {:?}", result);
        assert!(result.passed);
    }

    #[test]
    fn invariant_must_exist_fail() {
        let doc = parse(r#"(doc (p "Content"))"#);
        let inv = Invariant::MustExist {
            selector: "h1".to_string(),
        };
        let result = assert_invariant(&doc, &inv);
        assert!(!result.passed);
        assert_eq!(result.violations.len(), 1);
    }

    #[test]
    fn invariant_must_not_exist_pass() {
        let doc = parse(r#"(doc (h1 "Title"))"#);
        let inv = Invariant::MustNotExist {
            selector: "h6".to_string(),
        };
        let result = assert_invariant(&doc, &inv);
        assert!(result.passed);
    }

    #[test]
    fn invariant_must_not_exist_fail() {
        let doc = parse(r#"(doc (h6 "Deep heading"))"#);
        let inv = Invariant::MustNotExist {
            selector: "h6".to_string(),
        };
        let result = assert_invariant(&doc, &inv);
        assert!(!result.passed);
    }

    #[test]
    fn invariant_has_child_pass() {
        let doc = parse(r#"(doc (ul (li (p "Item"))))"#);
        let inv = Invariant::HasChild {
            selector: "li".to_string(),
            child_selector: "p".to_string(),
        };
        let result = assert_invariant(&doc, &inv);
        println!("DEBUG has_child result: {:?}", result);
        assert!(result.passed);
    }

    #[test]
    fn invariant_has_child_fail() {
        let doc = parse(r#"(doc (ul (li "No paragraph")))"#);
        let inv = Invariant::HasChild {
            selector: "li".to_string(),
            child_selector: "p".to_string(),
        };
        let result = assert_invariant(&doc, &inv);
        assert!(!result.passed);
    }

    #[test]
    fn invariant_max_depth_pass() {
        let doc = parse(r#"(doc (h1 "Title") (p "Text"))"#);
        let inv = Invariant::MaxDepth {
            selector: "h1".to_string(),
            max_depth: 2,
        };
        let result = assert_invariant(&doc, &inv);
        assert!(result.passed);
    }

    #[test]
    fn invariant_max_depth_fail() {
        let doc = parse(r#"(doc (blockquote (ul (li (h1 "Deep")))))"#);
        let inv = Invariant::MaxDepth {
            selector: "h1".to_string(),
            max_depth: 2,
        };
        let result = assert_invariant(&doc, &inv);
        println!("DEBUG max_depth result: {:?}", result);
        assert!(!result.passed);
    }

    #[test]
    fn invariant_no_heading_skips_pass() {
        let doc = parse(r#"(doc (h1 "A") (h2 "B") (h3 "C") (h2 "D"))"#);
        let result = assert_invariant(&doc, &Invariant::NoHeadingSkips);
        println!("DEBUG no_heading_skips result: {:?}", result);
        assert!(result.passed);
    }

    #[test]
    fn invariant_no_heading_skips_fail() {
        let doc = parse(r#"(doc (h1 "A") (h3 "Skipped h2"))"#);
        let result = assert_invariant(&doc, &Invariant::NoHeadingSkips);
        assert!(!result.passed);
        assert!(result.violations[0].message.contains("skipped"));
    }

    #[test]
    fn validate_all_mixed() {
        let doc = parse(r#"(doc (h1 "Title") (p "Text"))"#);
        let invariants = vec![
            Invariant::MustExist {
                selector: "h1".to_string(),
            },
            Invariant::MustNotExist {
                selector: "h6".to_string(),
            },
        ];
        let results = validate_all(&doc, &invariants);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.passed));
    }

    #[test]
    fn all_pass_true() {
        let doc = parse(r#"(doc (h1 "Title"))"#);
        let invariants = vec![Invariant::MustExist {
            selector: "h1".to_string(),
        }];
        assert!(all_pass(&doc, &invariants));
    }

    #[test]
    fn all_pass_false() {
        let doc = parse(r#"(doc (p "No heading"))"#);
        let invariants = vec![Invariant::MustExist {
            selector: "h1".to_string(),
        }];
        assert!(!all_pass(&doc, &invariants));
    }
}
