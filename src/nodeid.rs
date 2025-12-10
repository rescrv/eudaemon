//! Node identification system for markdown AST navigation.
//!
//! Provides two complementary addressing schemes:
//! - **Path-based IDs**: Positional path from root (e.g., "0.2.1" means root's child 0, its child 2, its child 1)
//! - **Content-hash IDs**: SHA3-256 hash of node content for content-addressable references
//!
//! Both ID types are printed when annotating nodes. Either can be used for selection.

use sha3::{Digest, Sha3_256};

use super::error::{SError, SResult};
use super::expr::SExpr;

/// A path-based node identifier.
/// Represents the index path from the document root to a specific node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathId(Vec<usize>);

impl PathId {
    /// Creates a new path ID from a vector of indices.
    pub fn new(path: Vec<usize>) -> Self {
        PathId(path)
    }

    /// Creates the root path ID.
    pub fn root() -> Self {
        PathId(vec![])
    }

    /// Creates a child path by appending an index.
    pub fn child(&self, index: usize) -> Self {
        let mut path = self.0.clone();
        path.push(index);
        PathId(path)
    }

    /// Returns the parent path, or None if this is the root.
    pub fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            None
        } else {
            let mut path = self.0.clone();
            path.pop();
            Some(PathId(path))
        }
    }

    /// Returns the depth of this path (0 for root).
    pub fn depth(&self) -> usize {
        self.0.len()
    }

    /// Returns the path as a slice of indices.
    pub fn indices(&self) -> &[usize] {
        &self.0
    }

    /// Parses a path ID from a string like "0.2.1" or "root".
    pub fn parse(s: &str) -> SResult<Self> {
        let s = s.trim();
        if s == "root" || s.is_empty() {
            return Ok(PathId::root());
        }

        let indices: Result<Vec<usize>, _> =
            s.split('.').map(|part| part.parse::<usize>()).collect();

        indices.map(PathId).map_err(|e| {
            SError::new("nodeid")
                .with_code("invalid-path-id")
                .with_message("Failed to parse path ID")
                .with_string_field("input", s)
                .with_string_field("parse_error", &e.to_string())
        })
    }
}

impl std::fmt::Display for PathId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            write!(f, "root")
        } else {
            let parts: Vec<String> = self.0.iter().map(|i| i.to_string()).collect();
            write!(f, "{}", parts.join("."))
        }
    }
}

/// Number of bytes used for content ID (12 hex chars = 6 bytes).
const CONTENT_ID_BYTES: usize = 6;

/// Number of hex characters in a content ID.
const CONTENT_ID_HEX_LEN: usize = CONTENT_ID_BYTES * 2;

/// A content-hash node identifier.
/// Uses the first 6 bytes (12 hex chars) of a SHA3-256 hash of the node's content.
/// The hash is stable across Rust versions and platforms.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentId([u8; CONTENT_ID_BYTES]);

impl ContentId {
    /// Computes the SHA3-256 content hash of an S-expression, keeping the first bytes.
    pub fn compute(expr: &SExpr) -> Self {
        let mut hasher = Sha3_256::new();
        Self::hash_expr(expr, &mut hasher);
        let result = hasher.finalize();
        let mut bytes = [0u8; CONTENT_ID_BYTES];
        bytes.copy_from_slice(&result[..CONTENT_ID_BYTES]);
        ContentId(bytes)
    }

    /// Recursively hashes an S-expression into the SHA3 hasher.
    fn hash_expr(expr: &SExpr, hasher: &mut Sha3_256) {
        match expr {
            SExpr::Atom(s) => {
                hasher.update([0u8]); // Type discriminant
                hasher.update((s.len() as u64).to_le_bytes());
                hasher.update(s.as_bytes());
            }
            SExpr::List(items) => {
                hasher.update([1u8]); // Type discriminant
                hasher.update((items.len() as u64).to_le_bytes());
                for item in items {
                    Self::hash_expr(item, hasher);
                }
            }
        }
    }

    /// Parses a content ID from a hex string prefixed with '#'.
    pub fn parse(s: &str) -> SResult<Self> {
        let s = s.trim();
        if !s.starts_with('#') {
            return Err(SError::new("nodeid")
                .with_code("invalid-content-id")
                .with_message("Content ID must start with '#'")
                .with_string_field("input", s));
        }

        let hex = &s[1..];
        if hex.len() != CONTENT_ID_HEX_LEN {
            return Err(SError::new("nodeid")
                .with_code("invalid-content-id")
                .with_message(&format!(
                    "Content ID must be {} hex characters",
                    CONTENT_ID_HEX_LEN
                ))
                .with_string_field("input", s)
                .with_atom_field("length", hex.len()));
        }

        let mut bytes = [0u8; CONTENT_ID_BYTES];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let hex_str = std::str::from_utf8(chunk).map_err(|e| {
                SError::new("nodeid")
                    .with_code("invalid-content-id")
                    .with_message("Invalid UTF-8 in hex string")
                    .with_string_field("parse_error", &e.to_string())
            })?;
            bytes[i] = u8::from_str_radix(hex_str, 16).map_err(|e| {
                SError::new("nodeid")
                    .with_code("invalid-content-id")
                    .with_message("Failed to parse content ID hex value")
                    .with_string_field("input", s)
                    .with_string_field("parse_error", &e.to_string())
            })?;
        }
        Ok(ContentId(bytes))
    }
}

impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#")?;
        for byte in &self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

/// A node identifier that can be either path-based or content-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeId {
    /// A positional identifier specifying the index path from the document root.
    Path(PathId),
    /// A content-addressable identifier based on the node's structural hash.
    Content(ContentId),
}

impl NodeId {
    /// Parses a node ID from a string.
    /// - Strings starting with '#' are parsed as content IDs
    /// - Everything else is parsed as a path ID
    pub fn parse(s: &str) -> SResult<Self> {
        let s = s.trim();
        if s.starts_with('#') {
            ContentId::parse(s).map(NodeId::Content)
        } else {
            PathId::parse(s).map(NodeId::Path)
        }
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeId::Path(p) => write!(f, "{}", p),
            NodeId::Content(c) => write!(f, "{}", c),
        }
    }
}

/// Annotated node containing both ID types and the node data.
#[derive(Debug, Clone)]
pub struct AnnotatedNode {
    /// Path-based identifier.
    pub path_id: PathId,
    /// Content-hash identifier.
    pub content_id: ContentId,
    /// The node's S-expression.
    pub node: SExpr,
}

/// Annotates all nodes in a document with their IDs.
/// Returns a flat list of all annotated nodes.
pub fn annotate_document(doc: &SExpr) -> Vec<AnnotatedNode> {
    let mut nodes = Vec::new();
    annotate_recursive(doc, PathId::root(), &mut nodes);
    nodes
}

/// Recursively annotates nodes.
fn annotate_recursive(expr: &SExpr, path: PathId, nodes: &mut Vec<AnnotatedNode>) {
    let content_id = ContentId::compute(expr);
    nodes.push(AnnotatedNode {
        path_id: path.clone(),
        content_id,
        node: expr.clone(),
    });

    if let SExpr::List(items) = expr {
        // Skip the tag (first element) when annotating children
        for (i, child) in items.iter().enumerate().skip(1) {
            annotate_recursive(child, path.child(i), nodes);
        }
    }
}

/// Gets a node by its path ID.
pub fn get_by_path(doc: &SExpr, path: &PathId) -> Option<SExpr> {
    let mut current = doc;
    for &index in path.indices() {
        match current {
            SExpr::List(items) => {
                current = items.get(index)?;
            }
            SExpr::Atom(_) => return None,
        }
    }
    Some(current.clone())
}

/// Gets a node by its content ID.
/// Returns the first matching node if multiple have the same content.
pub fn get_by_content(doc: &SExpr, content_id: &ContentId) -> Option<(PathId, SExpr)> {
    let nodes = annotate_document(doc);
    nodes
        .into_iter()
        .find(|n| &n.content_id == content_id)
        .map(|n| (n.path_id, n.node))
}

/// Gets a node by either path or content ID.
pub fn get_node(doc: &SExpr, id: &NodeId) -> Option<(PathId, SExpr)> {
    match id {
        NodeId::Path(path) => get_by_path(doc, path).map(|node| (path.clone(), node)),
        NodeId::Content(content) => get_by_content(doc, content),
    }
}

/// Returns the parent of a node identified by path.
pub fn get_parent(doc: &SExpr, path: &PathId) -> Option<(PathId, SExpr)> {
    let parent_path = path.parent()?;
    get_by_path(doc, &parent_path).map(|node| (parent_path, node))
}

/// Returns the siblings of a node (nodes at the same level with the same parent).
pub fn get_siblings(doc: &SExpr, path: &PathId) -> Vec<(PathId, SExpr)> {
    let parent_path = match path.parent() {
        Some(p) => p,
        None => return vec![], // Root has no siblings
    };

    let parent = match get_by_path(doc, &parent_path) {
        Some(p) => p,
        None => return vec![],
    };

    match parent {
        SExpr::List(items) => items
            .iter()
            .enumerate()
            .skip(1) // Skip tag
            .map(|(i, child)| (parent_path.child(i), child.clone()))
            .collect(),
        SExpr::Atom(_) => vec![],
    }
}

/// Returns surrounding context (siblings before and after) for a node.
pub fn get_context(doc: &SExpr, path: &PathId, radius: usize) -> Vec<(PathId, SExpr)> {
    let siblings = get_siblings(doc, path);

    // Find our position among siblings
    let position = siblings.iter().position(|(p, _)| p == path);
    let position = match position {
        Some(p) => p,
        None => return vec![],
    };

    let start = position.saturating_sub(radius);
    let end = (position + radius + 1).min(siblings.len());

    siblings[start..end].to_vec()
}

/// Converts a document to an annotated S-expression with ID attributes.
/// Each node is wrapped with its IDs: (@ path-id content-id node)
pub fn to_annotated_sexpr(doc: &SExpr) -> SExpr {
    annotate_sexpr_recursive(doc, PathId::root())
}

/// Recursively builds annotated S-expression.
fn annotate_sexpr_recursive(expr: &SExpr, path: PathId) -> SExpr {
    let content_id = ContentId::compute(expr);

    match expr {
        SExpr::Atom(_) => SExpr::List(vec![
            SExpr::Atom("@".to_string()),
            SExpr::Atom(format!("\"{}\"", path)),
            SExpr::Atom(format!("\"{}\"", content_id)),
            expr.clone(),
        ]),
        SExpr::List(items) => {
            if items.is_empty() {
                return SExpr::List(vec![
                    SExpr::Atom("@".to_string()),
                    SExpr::Atom(format!("\"{}\"", path)),
                    SExpr::Atom(format!("\"{}\"", content_id)),
                    expr.clone(),
                ]);
            }

            // Annotate children (skip tag)
            let mut annotated_items = vec![items[0].clone()];
            for (i, child) in items.iter().enumerate().skip(1) {
                annotated_items.push(annotate_sexpr_recursive(child, path.child(i)));
            }

            SExpr::List(vec![
                SExpr::Atom("@".to_string()),
                SExpr::Atom(format!("\"{}\"", path)),
                SExpr::Atom(format!("\"{}\"", content_id)),
                SExpr::List(annotated_items),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Parser;

    fn parse(s: &str) -> SExpr {
        Parser::new(s).parse().unwrap()
    }

    #[test]
    fn path_id_display() {
        assert_eq!(PathId::root().to_string(), "root");
        assert_eq!(PathId::new(vec![0]).to_string(), "0");
        assert_eq!(PathId::new(vec![0, 2, 1]).to_string(), "0.2.1");
    }

    #[test]
    fn path_id_parse() {
        assert_eq!(PathId::parse("root").unwrap(), PathId::root());
        assert_eq!(PathId::parse("").unwrap(), PathId::root());
        assert_eq!(PathId::parse("0").unwrap(), PathId::new(vec![0]));
        assert_eq!(PathId::parse("0.2.1").unwrap(), PathId::new(vec![0, 2, 1]));
    }

    #[test]
    fn path_id_child_parent() {
        let root = PathId::root();
        let child = root.child(0);
        let grandchild = child.child(2);

        assert_eq!(child, PathId::new(vec![0]));
        assert_eq!(grandchild, PathId::new(vec![0, 2]));
        assert_eq!(grandchild.parent(), Some(child.clone()));
        assert_eq!(child.parent(), Some(root.clone()));
        assert_eq!(root.parent(), None);
    }

    #[test]
    fn content_id_consistency() {
        let expr1 = parse("(doc (h1 \"Hello\"))");
        let expr2 = parse("(doc (h1 \"Hello\"))");
        let expr3 = parse("(doc (h1 \"World\"))");

        let id1 = ContentId::compute(&expr1);
        let id2 = ContentId::compute(&expr2);
        let id3 = ContentId::compute(&expr3);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn content_id_display_parse() {
        let expr = parse("(doc (h1 \"Hello\"))");
        let id = ContentId::compute(&expr);
        let display = id.to_string();

        assert!(display.starts_with('#'));
        let parsed = ContentId::parse(&display).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn node_id_parse() {
        let path = NodeId::parse("0.2.1").unwrap();
        assert!(matches!(path, NodeId::Path(_)));

        let content = NodeId::parse("#012345678901").unwrap();
        assert!(matches!(content, NodeId::Content(_)));
    }

    #[test]
    fn get_by_path_basic() {
        let doc = parse("(doc (h1 \"Title\") (p \"Content\"))");

        // Root
        let root = get_by_path(&doc, &PathId::root()).unwrap();
        assert_eq!(root, doc);

        // First child (h1)
        let h1 = get_by_path(&doc, &PathId::new(vec![1])).unwrap();
        assert_eq!(h1.to_string(), "(h1 \"Title\")");

        // Second child (p)
        let p = get_by_path(&doc, &PathId::new(vec![2])).unwrap();
        assert_eq!(p.to_string(), "(p \"Content\")");

        // Nested: h1's text
        let title = get_by_path(&doc, &PathId::new(vec![1, 1])).unwrap();
        assert_eq!(title.to_string(), "\"Title\"");
    }

    #[test]
    fn get_by_content_basic() {
        let doc = parse("(doc (h1 \"Title\") (p \"Content\"))");
        let h1 = parse("(h1 \"Title\")");

        let h1_id = ContentId::compute(&h1);
        let result = get_by_content(&doc, &h1_id);

        assert!(result.is_some());
        let (path, node) = result.unwrap();
        assert_eq!(path, PathId::new(vec![1]));
        assert_eq!(node.to_string(), "(h1 \"Title\")");
    }

    #[test]
    fn annotate_document_basic() {
        let doc = parse("(doc (h1 \"Title\"))");
        let nodes = annotate_document(&doc);

        // Should have: doc, h1, "Title"
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].path_id, PathId::root());
        assert_eq!(nodes[1].path_id, PathId::new(vec![1]));
        assert_eq!(nodes[2].path_id, PathId::new(vec![1, 1]));
    }

    #[test]
    fn get_siblings_basic() {
        let doc = parse("(doc (h1 \"A\") (p \"B\") (p \"C\"))");
        let path = PathId::new(vec![2]); // Second p

        let siblings = get_siblings(&doc, &path);
        assert_eq!(siblings.len(), 3);
    }

    #[test]
    fn get_context_basic() {
        let doc = parse("(doc (p \"1\") (p \"2\") (p \"3\") (p \"4\") (p \"5\"))");
        let path = PathId::new(vec![3]); // p "3"

        let context = get_context(&doc, &path, 1);
        // Should get p "2", p "3", p "4"
        assert_eq!(context.len(), 3);
    }

    #[test]
    fn to_annotated_sexpr_basic() {
        let doc = parse("(doc (h1 \"Title\"))");
        let annotated = to_annotated_sexpr(&doc);
        let s = annotated.to_string();

        assert!(s.contains("(@"));
        assert!(s.contains("\"root\""));
        assert!(s.contains("\"1\""));
    }

    #[test]
    fn path_id_depth() {
        assert_eq!(PathId::root().depth(), 0);
        assert_eq!(PathId::new(vec![1]).depth(), 1);
        assert_eq!(PathId::new(vec![1, 2, 3]).depth(), 3);
    }

    #[test]
    fn path_id_parse_invalid_error() {
        let err = PathId::parse("not.a.number").unwrap_err();
        eprintln!("DEBUG path_id_parse_invalid_error: {}", err.detail());
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("nodeid".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("invalid-path-id".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"Failed to parse path ID\"".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("input".to_string()),
                    SExpr::Atom("\"not.a.number\"".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("parse_error".to_string()),
                    SExpr::Atom("\"invalid digit found in string\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn content_id_parse_invalid_error() {
        let err = ContentId::parse("no-hash-prefix").unwrap_err();
        eprintln!("DEBUG content_id_parse_invalid_error: {}", err.detail());
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("nodeid".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("invalid-content-id".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"Content ID must start with '#'\"".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("input".to_string()),
                    SExpr::Atom("\"no-hash-prefix\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn get_by_path_invalid_index_returns_none() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = get_by_path(&doc, &PathId::new(vec![99]));
        assert!(result.is_none());
    }

    #[test]
    fn get_by_path_into_atom_returns_none() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = get_by_path(&doc, &PathId::new(vec![1, 1, 1]));
        assert!(result.is_none());
    }

    #[test]
    fn get_by_content_not_found_returns_none() {
        let doc = parse("(doc (h1 \"Title\"))");
        let nonexistent = ContentId::compute(&parse("(p \"Not in doc\")"));
        let result = get_by_content(&doc, &nonexistent);
        assert!(result.is_none());
    }

    #[test]
    fn get_node_with_path() {
        let doc = parse("(doc (h1 \"Title\"))");
        let id = NodeId::Path(PathId::new(vec![1]));
        let result = get_node(&doc, &id);
        assert!(result.is_some());
        assert_eq!(result.unwrap().1.to_string(), "(h1 \"Title\")");
    }

    #[test]
    fn get_node_with_content() {
        let doc = parse("(doc (h1 \"Title\"))");
        let h1 = parse("(h1 \"Title\")");
        let id = NodeId::Content(ContentId::compute(&h1));
        let result = get_node(&doc, &id);
        assert!(result.is_some());
        assert_eq!(result.unwrap().1.to_string(), "(h1 \"Title\")");
    }

    #[test]
    fn get_parent_of_root_returns_none() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = get_parent(&doc, &PathId::root());
        assert!(result.is_none());
    }

    #[test]
    fn get_parent_of_child() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = get_parent(&doc, &PathId::new(vec![1]));
        assert!(result.is_some());
        let (parent_path, parent_node) = result.unwrap();
        assert_eq!(parent_path, PathId::root());
        assert_eq!(parent_node, doc);
    }

    #[test]
    fn get_siblings_of_root_returns_empty() {
        let doc = parse("(doc (h1 \"Title\"))");
        let siblings = get_siblings(&doc, &PathId::root());
        assert!(siblings.is_empty());
    }

    #[test]
    fn get_siblings_invalid_parent_returns_empty() {
        let doc = parse("(doc (h1 \"Title\"))");
        let siblings = get_siblings(&doc, &PathId::new(vec![99, 1]));
        assert!(siblings.is_empty());
    }

    #[test]
    fn get_context_at_boundary_start() {
        let doc = parse("(doc (p \"1\") (p \"2\") (p \"3\"))");
        let path = PathId::new(vec![1]);
        let context = get_context(&doc, &path, 1);
        assert_eq!(context.len(), 2);
    }

    #[test]
    fn get_context_at_boundary_end() {
        let doc = parse("(doc (p \"1\") (p \"2\") (p \"3\"))");
        let path = PathId::new(vec![3]);
        let context = get_context(&doc, &path, 1);
        assert_eq!(context.len(), 2);
    }

    #[test]
    fn get_context_not_found_returns_empty() {
        let doc = parse("(doc (p \"1\") (p \"2\"))");
        let path = PathId::new(vec![99]);
        let context = get_context(&doc, &path, 1);
        assert!(context.is_empty());
    }

    #[test]
    fn annotate_document_deeply_nested() {
        let doc = parse("(doc (ul (li (p \"Deep\"))))");
        let nodes = annotate_document(&doc);
        // doc, ul, li, p, "Deep" = 5 nodes
        assert_eq!(nodes.len(), 5);
    }

    #[test]
    fn annotate_document_empty_list() {
        let doc = parse("()");
        let nodes = annotate_document(&doc);
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn to_annotated_sexpr_empty_list() {
        let doc = parse("()");
        let annotated = to_annotated_sexpr(&doc);
        let s = annotated.to_string();
        assert!(s.contains("(@"));
    }

    #[test]
    fn node_id_display_path() {
        let id = NodeId::Path(PathId::new(vec![1, 2]));
        assert_eq!(id.to_string(), "1.2");
    }

    #[test]
    fn node_id_display_content() {
        let expr = parse("(h1 \"Test\")");
        let content_id = ContentId::compute(&expr);
        let id = NodeId::Content(content_id.clone());
        assert_eq!(id.to_string(), content_id.to_string());
    }

    #[test]
    fn content_id_deterministic_across_calls() {
        let expr = parse("(p \"Content\")");
        let id1 = ContentId::compute(&expr);
        let id2 = ContentId::compute(&expr);
        let id3 = ContentId::compute(&expr);
        assert_eq!(id1, id2);
        assert_eq!(id2, id3);
    }

    #[test]
    fn path_id_equality() {
        let p1 = PathId::new(vec![1, 2, 3]);
        let p2 = PathId::new(vec![1, 2, 3]);
        let p3 = PathId::new(vec![1, 2, 4]);
        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }
}
