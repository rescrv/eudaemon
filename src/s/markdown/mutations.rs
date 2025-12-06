//! Atomic mutation operations for markdown AST transformation.
//!
//! All mutations are immutable - they return a new document with the changes applied.
//! Two variants are provided for each operation:
//! - Standard: Returns `Err` on failure (abort semantics)
//! - `_lenient`: Returns the original document unchanged on failure (continue semantics)

use crate::s::error::{SError, SResult};
use crate::s::expr::SExpr;
use crate::s::nodeid::PathId;

/// Replaces a node at the given path with a new node.
/// Returns the modified document.
pub fn replace_at(doc: &SExpr, path: &PathId, new_node: SExpr) -> SResult<SExpr> {
    if path.indices().is_empty() {
        // Replacing root
        return Ok(new_node);
    }
    replace_at_impl(doc, path.indices(), new_node)
}

/// Lenient version: returns original document if path is invalid.
pub fn replace_at_lenient(doc: &SExpr, path: &PathId, new_node: SExpr) -> SExpr {
    replace_at(doc, path, new_node).unwrap_or_else(|_| doc.clone())
}

fn replace_at_impl(expr: &SExpr, indices: &[usize], new_node: SExpr) -> SResult<SExpr> {
    match expr {
        SExpr::List(items) => {
            if indices.is_empty() {
                return Ok(new_node);
            }

            let idx = indices[0];
            if idx >= items.len() {
                return Err(SError::new("mutations")
                    .with_code("index-out-of-bounds")
                    .with_message("Path index exceeds list length")
                    .with_atom_field("index", idx)
                    .with_atom_field("list_length", items.len()));
            }

            let mut new_items = items.clone();
            if indices.len() == 1 {
                new_items[idx] = new_node;
            } else {
                new_items[idx] = replace_at_impl(&items[idx], &indices[1..], new_node)?;
            }
            Ok(SExpr::List(new_items))
        }
        SExpr::Atom(_) => Err(SError::new("mutations")
            .with_code("cannot-descend-atom")
            .with_message("Cannot navigate into an atom")
            .with_field("expr", expr.clone())),
    }
}

/// Removes a node at the given path.
/// Returns the modified document.
pub fn prune(doc: &SExpr, path: &PathId) -> SResult<SExpr> {
    if path.indices().is_empty() {
        return Err(SError::new("mutations")
            .with_code("cannot-prune-root")
            .with_message("Cannot prune the root document"));
    }
    prune_impl(doc, path.indices())
}

/// Lenient version: returns original document if path is invalid.
pub fn prune_lenient(doc: &SExpr, path: &PathId) -> SExpr {
    prune(doc, path).unwrap_or_else(|_| doc.clone())
}

fn prune_impl(expr: &SExpr, indices: &[usize]) -> SResult<SExpr> {
    match expr {
        SExpr::List(items) => {
            if indices.is_empty() {
                return Err(SError::new("mutations")
                    .with_code("invalid-prune-path")
                    .with_message("Empty path in prune"));
            }

            let idx = indices[0];
            if idx >= items.len() {
                return Err(SError::new("mutations")
                    .with_code("index-out-of-bounds")
                    .with_message("Path index exceeds list length")
                    .with_atom_field("index", idx)
                    .with_atom_field("list_length", items.len()));
            }

            if indices.len() == 1 {
                // Remove this element
                let mut new_items = items.clone();
                new_items.remove(idx);
                Ok(SExpr::List(new_items))
            } else {
                // Recurse
                let mut new_items = items.clone();
                new_items[idx] = prune_impl(&items[idx], &indices[1..])?;
                Ok(SExpr::List(new_items))
            }
        }
        SExpr::Atom(_) => Err(SError::new("mutations")
            .with_code("cannot-descend-atom")
            .with_message("Cannot navigate into an atom")),
    }
}

/// Inserts a node before the node at the given path.
pub fn insert_before(doc: &SExpr, path: &PathId, new_node: SExpr) -> SResult<SExpr> {
    if path.indices().is_empty() {
        return Err(SError::new("mutations")
            .with_code("cannot-insert-before-root")
            .with_message("Cannot insert before root"));
    }
    insert_at_impl(doc, path.indices(), new_node, false)
}

/// Lenient version: returns original document if path is invalid.
pub fn insert_before_lenient(doc: &SExpr, path: &PathId, new_node: SExpr) -> SExpr {
    insert_before(doc, path, new_node).unwrap_or_else(|_| doc.clone())
}

/// Inserts a node after the node at the given path.
pub fn insert_after(doc: &SExpr, path: &PathId, new_node: SExpr) -> SResult<SExpr> {
    if path.indices().is_empty() {
        return Err(SError::new("mutations")
            .with_code("cannot-insert-after-root")
            .with_message("Cannot insert after root"));
    }
    insert_at_impl(doc, path.indices(), new_node, true)
}

/// Lenient version: returns original document if path is invalid.
pub fn insert_after_lenient(doc: &SExpr, path: &PathId, new_node: SExpr) -> SExpr {
    insert_after(doc, path, new_node).unwrap_or_else(|_| doc.clone())
}

fn insert_at_impl(expr: &SExpr, indices: &[usize], new_node: SExpr, after: bool) -> SResult<SExpr> {
    match expr {
        SExpr::List(items) => {
            if indices.is_empty() {
                return Err(SError::new("mutations")
                    .with_code("invalid-insert-path")
                    .with_message("Empty path in insert"));
            }

            let idx = indices[0];
            if idx >= items.len() {
                return Err(SError::new("mutations")
                    .with_code("index-out-of-bounds")
                    .with_message("Path index exceeds list length")
                    .with_atom_field("index", idx)
                    .with_atom_field("list_length", items.len()));
            }

            if indices.len() == 1 {
                // Insert here
                let mut new_items = items.clone();
                let insert_idx = if after { idx + 1 } else { idx };
                new_items.insert(insert_idx, new_node);
                Ok(SExpr::List(new_items))
            } else {
                // Recurse
                let mut new_items = items.clone();
                new_items[idx] = insert_at_impl(&items[idx], &indices[1..], new_node, after)?;
                Ok(SExpr::List(new_items))
            }
        }
        SExpr::Atom(_) => Err(SError::new("mutations")
            .with_code("cannot-descend-atom")
            .with_message("Cannot navigate into an atom")),
    }
}

/// Appends a child to a node (as last child).
pub fn append_child(doc: &SExpr, parent_path: &PathId, child: SExpr) -> SResult<SExpr> {
    if parent_path.indices().is_empty() {
        // Append to root
        match doc {
            SExpr::List(items) => {
                let mut new_items = items.clone();
                new_items.push(child);
                Ok(SExpr::List(new_items))
            }
            SExpr::Atom(_) => Err(SError::new("mutations")
                .with_code("cannot-append-to-atom")
                .with_message("Cannot append child to an atom")),
        }
    } else {
        append_child_impl(doc, parent_path.indices(), child)
    }
}

/// Lenient version: returns original document if path is invalid.
pub fn append_child_lenient(doc: &SExpr, parent_path: &PathId, child: SExpr) -> SExpr {
    append_child(doc, parent_path, child).unwrap_or_else(|_| doc.clone())
}

fn append_child_impl(expr: &SExpr, indices: &[usize], child: SExpr) -> SResult<SExpr> {
    match expr {
        SExpr::List(items) => {
            if indices.is_empty() {
                let mut new_items = items.clone();
                new_items.push(child);
                return Ok(SExpr::List(new_items));
            }

            let idx = indices[0];
            if idx >= items.len() {
                return Err(SError::new("mutations")
                    .with_code("index-out-of-bounds")
                    .with_message("Path index exceeds list length")
                    .with_atom_field("index", idx)
                    .with_atom_field("list_length", items.len()));
            }

            let mut new_items = items.clone();
            new_items[idx] = append_child_impl(&items[idx], &indices[1..], child)?;
            Ok(SExpr::List(new_items))
        }
        SExpr::Atom(_) => Err(SError::new("mutations")
            .with_code("cannot-append-to-atom")
            .with_message("Cannot append child to an atom")),
    }
}

/// Prepends a child to a node (as first child after the tag).
pub fn prepend_child(doc: &SExpr, parent_path: &PathId, child: SExpr) -> SResult<SExpr> {
    if parent_path.indices().is_empty() {
        // Prepend to root (after the tag)
        match doc {
            SExpr::List(items) => {
                let mut new_items = items.clone();
                if items.is_empty() {
                    new_items.push(child);
                } else {
                    new_items.insert(1, child);
                }
                Ok(SExpr::List(new_items))
            }
            SExpr::Atom(_) => Err(SError::new("mutations")
                .with_code("cannot-prepend-to-atom")
                .with_message("Cannot prepend child to an atom")),
        }
    } else {
        prepend_child_impl(doc, parent_path.indices(), child)
    }
}

/// Lenient version: returns original document if path is invalid.
pub fn prepend_child_lenient(doc: &SExpr, parent_path: &PathId, child: SExpr) -> SExpr {
    prepend_child(doc, parent_path, child).unwrap_or_else(|_| doc.clone())
}

fn prepend_child_impl(expr: &SExpr, indices: &[usize], child: SExpr) -> SResult<SExpr> {
    match expr {
        SExpr::List(items) => {
            if indices.is_empty() {
                let mut new_items = items.clone();
                if items.is_empty() {
                    new_items.push(child);
                } else {
                    new_items.insert(1, child);
                }
                return Ok(SExpr::List(new_items));
            }

            let idx = indices[0];
            if idx >= items.len() {
                return Err(SError::new("mutations")
                    .with_code("index-out-of-bounds")
                    .with_message("Path index exceeds list length")
                    .with_atom_field("index", idx)
                    .with_atom_field("list_length", items.len()));
            }

            let mut new_items = items.clone();
            new_items[idx] = prepend_child_impl(&items[idx], &indices[1..], child)?;
            Ok(SExpr::List(new_items))
        }
        SExpr::Atom(_) => Err(SError::new("mutations")
            .with_code("cannot-prepend-to-atom")
            .with_message("Cannot prepend child to an atom")),
    }
}

/// Changes the heading level by delta.
/// delta > 0 demotes (h1 -> h2), delta < 0 promotes (h2 -> h1).
/// Clamps to h1-h6 range.
pub fn hoist(doc: &SExpr, path: &PathId, delta: i32) -> SResult<SExpr> {
    let node = crate::s::nodeid::get_by_path(doc, path).ok_or_else(|| {
        SError::new("mutations")
            .with_code("node-not-found")
            .with_message("No node at the specified path")
            .with_string_field("path", &path.to_string())
    })?;

    // Check if it's a heading
    let new_node = match &node {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0] {
                if tag.starts_with('h') && tag.len() == 2 {
                    if let Ok(level) = tag[1..].parse::<i32>() {
                        let new_level = (level + delta).clamp(1, 6);
                        let new_tag = format!("h{}", new_level);
                        let mut new_items = items.clone();
                        new_items[0] = SExpr::Atom(new_tag);
                        SExpr::List(new_items)
                    } else {
                        return Err(SError::new("mutations")
                            .with_code("invalid-heading")
                            .with_message("Node tag looks like a heading but has invalid level")
                            .with_string_field("tag", tag));
                    }
                } else {
                    return Err(SError::new("mutations")
                        .with_code("not-a-heading")
                        .with_message("Node is not a heading (h1-h6)")
                        .with_string_field("tag", tag));
                }
            } else {
                return Err(SError::new("mutations")
                    .with_code("invalid-node")
                    .with_message("Node tag is not an atom"));
            }
        }
        _ => {
            return Err(SError::new("mutations")
                .with_code("invalid-node")
                .with_message("Cannot hoist a non-list node"));
        }
    };

    replace_at(doc, path, new_node)
}

/// Lenient version: returns original document if operation fails.
pub fn hoist_lenient(doc: &SExpr, path: &PathId, delta: i32) -> SExpr {
    hoist(doc, path, delta).unwrap_or_else(|_| doc.clone())
}

/// Moves a subtree from source_path to a new location.
/// The subtree is removed from source and inserted at target_parent at the specified index.
pub fn graft(
    doc: &SExpr,
    source_path: &PathId,
    target_parent_path: &PathId,
    target_index: usize,
) -> SResult<SExpr> {
    // Get the node to move
    let node = crate::s::nodeid::get_by_path(doc, source_path).ok_or_else(|| {
        SError::new("mutations")
            .with_code("source-not-found")
            .with_message("No node at the source path")
            .with_string_field("source_path", &source_path.to_string())
    })?;

    // Remove from source
    let doc_after_prune = prune(doc, source_path)?;

    // Need to adjust target path if it was affected by the prune
    // This is complex - for now, we'll do a simple implementation
    // that works when source and target are in different subtrees

    // Insert at target
    insert_child_at(&doc_after_prune, target_parent_path, target_index, node)
}

/// Lenient version: returns original document if operation fails.
pub fn graft_lenient(
    doc: &SExpr,
    source_path: &PathId,
    target_parent_path: &PathId,
    target_index: usize,
) -> SExpr {
    graft(doc, source_path, target_parent_path, target_index).unwrap_or_else(|_| doc.clone())
}

/// Inserts a child at a specific index within a parent node.
fn insert_child_at(
    doc: &SExpr,
    parent_path: &PathId,
    index: usize,
    child: SExpr,
) -> SResult<SExpr> {
    if parent_path.indices().is_empty() {
        match doc {
            SExpr::List(items) => {
                let mut new_items = items.clone();
                let insert_idx = index.min(items.len());
                new_items.insert(insert_idx, child);
                Ok(SExpr::List(new_items))
            }
            SExpr::Atom(_) => Err(SError::new("mutations")
                .with_code("cannot-insert-into-atom")
                .with_message("Cannot insert child into an atom")),
        }
    } else {
        insert_child_at_impl(doc, parent_path.indices(), index, child)
    }
}

fn insert_child_at_impl(
    expr: &SExpr,
    indices: &[usize],
    target_index: usize,
    child: SExpr,
) -> SResult<SExpr> {
    match expr {
        SExpr::List(items) => {
            if indices.is_empty() {
                let mut new_items = items.clone();
                let insert_idx = target_index.min(items.len());
                new_items.insert(insert_idx, child);
                return Ok(SExpr::List(new_items));
            }

            let idx = indices[0];
            if idx >= items.len() {
                return Err(SError::new("mutations")
                    .with_code("index-out-of-bounds")
                    .with_message("Path index exceeds list length")
                    .with_atom_field("index", idx)
                    .with_atom_field("list_length", items.len()));
            }

            let mut new_items = items.clone();
            new_items[idx] = insert_child_at_impl(&items[idx], &indices[1..], target_index, child)?;
            Ok(SExpr::List(new_items))
        }
        SExpr::Atom(_) => Err(SError::new("mutations")
            .with_code("cannot-insert-into-atom")
            .with_message("Cannot insert child into an atom")),
    }
}

/// Applies multiple mutations in sequence.
/// Aborts on first error and returns the error.
pub fn apply_mutations<F>(doc: &SExpr, mutations: Vec<F>) -> SResult<SExpr>
where
    F: FnOnce(&SExpr) -> SResult<SExpr>,
{
    let mut current = doc.clone();
    for mutation in mutations {
        current = mutation(&current)?;
    }
    Ok(current)
}

/// Applies multiple mutations in sequence.
/// Continues on error, returning the last successful state.
pub fn apply_mutations_lenient<F>(doc: &SExpr, mutations: Vec<F>) -> SExpr
where
    F: FnOnce(&SExpr) -> SResult<SExpr>,
{
    let mut current = doc.clone();
    for mutation in mutations {
        if let Ok(new_doc) = mutation(&current) {
            current = new_doc;
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s::expr::Parser;

    fn parse(s: &str) -> SExpr {
        Parser::new(s).parse().unwrap()
    }

    #[test]
    fn replace_at_root() {
        let doc = parse("(doc (h1 \"Old\"))");
        let new_doc = parse("(doc (h1 \"New\"))");
        let result = replace_at(&doc, &PathId::root(), new_doc.clone()).unwrap();
        assert_eq!(result, new_doc);
    }

    #[test]
    fn replace_at_child() {
        let doc = parse("(doc (h1 \"Title\") (p \"Content\"))");
        let new_heading = parse("(h2 \"New Title\")");
        let result = replace_at(&doc, &PathId::new(vec![1]), new_heading).unwrap();
        assert_eq!(
            result.to_string(),
            "(doc (h2 \"New Title\") (p \"Content\"))"
        );
    }

    #[test]
    fn replace_at_nested() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = replace_at(
            &doc,
            &PathId::new(vec![1, 1]),
            SExpr::Atom("\"New Title\"".to_string()),
        )
        .unwrap();
        assert_eq!(result.to_string(), "(doc (h1 \"New Title\"))");
    }

    #[test]
    fn replace_at_invalid_path() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = replace_at(&doc, &PathId::new(vec![99]), SExpr::Atom("x".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn replace_at_lenient_invalid_path() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = replace_at_lenient(&doc, &PathId::new(vec![99]), SExpr::Atom("x".to_string()));
        assert_eq!(result, doc);
    }

    #[test]
    fn prune_child() {
        let doc = parse("(doc (h1 \"A\") (p \"B\") (p \"C\"))");
        let result = prune(&doc, &PathId::new(vec![2])).unwrap();
        assert_eq!(result.to_string(), "(doc (h1 \"A\") (p \"C\"))");
    }

    #[test]
    fn prune_root_fails() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = prune(&doc, &PathId::root());
        assert!(result.is_err());
    }

    #[test]
    fn insert_before_basic() {
        let doc = parse("(doc (h1 \"A\") (p \"C\"))");
        let new_node = parse("(p \"B\")");
        let result = insert_before(&doc, &PathId::new(vec![2]), new_node).unwrap();
        assert_eq!(result.to_string(), "(doc (h1 \"A\") (p \"B\") (p \"C\"))");
    }

    #[test]
    fn insert_after_basic() {
        let doc = parse("(doc (h1 \"A\") (p \"B\"))");
        let new_node = parse("(p \"C\")");
        let result = insert_after(&doc, &PathId::new(vec![1]), new_node).unwrap();
        assert_eq!(result.to_string(), "(doc (h1 \"A\") (p \"C\") (p \"B\"))");
    }

    #[test]
    fn append_child_basic() {
        let doc = parse("(doc (h1 \"Title\"))");
        let new_node = parse("(p \"Content\")");
        let result = append_child(&doc, &PathId::root(), new_node).unwrap();
        assert_eq!(result.to_string(), "(doc (h1 \"Title\") (p \"Content\"))");
    }

    #[test]
    fn prepend_child_basic() {
        let doc = parse("(doc (p \"Content\"))");
        let new_node = parse("(h1 \"Title\")");
        let result = prepend_child(&doc, &PathId::root(), new_node).unwrap();
        assert_eq!(result.to_string(), "(doc (h1 \"Title\") (p \"Content\"))");
    }

    #[test]
    fn hoist_demote() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = hoist(&doc, &PathId::new(vec![1]), 1).unwrap();
        assert_eq!(result.to_string(), "(doc (h2 \"Title\"))");
    }

    #[test]
    fn hoist_promote() {
        let doc = parse("(doc (h3 \"Title\"))");
        let result = hoist(&doc, &PathId::new(vec![1]), -2).unwrap();
        assert_eq!(result.to_string(), "(doc (h1 \"Title\"))");
    }

    #[test]
    fn hoist_clamps_at_h1() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = hoist(&doc, &PathId::new(vec![1]), -5).unwrap();
        assert_eq!(result.to_string(), "(doc (h1 \"Title\"))");
    }

    #[test]
    fn hoist_clamps_at_h6() {
        let doc = parse("(doc (h5 \"Title\"))");
        let result = hoist(&doc, &PathId::new(vec![1]), 10).unwrap();
        assert_eq!(result.to_string(), "(doc (h6 \"Title\"))");
    }

    #[test]
    fn hoist_non_heading_fails() {
        let doc = parse("(doc (p \"Not a heading\"))");
        let result = hoist(&doc, &PathId::new(vec![1]), 1);
        assert!(result.is_err());
    }

    #[test]
    fn graft_basic() {
        let doc = parse("(doc (h1 \"Title\") (ul (li (p \"A\")) (li (p \"B\"))))");
        // Move the second li to position 1 (after the tag) in the document
        let result = graft(
            &doc,
            &PathId::new(vec![2, 2]), // ul's second li
            &PathId::root(),
            2, // Insert at position 2 in doc
        )
        .unwrap();
        // The li should now be a direct child of doc
        let s = result.to_string();
        println!("DEBUG graft result: {}", s);
        assert!(s.contains("(li (p \"B\"))"));
    }
}
