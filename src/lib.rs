//! A toolkit for transforming markdown documents using S-expressions.
//!
//! `agentkb` provides a bidirectional transformation layer between markdown and
//! S-expressions, enabling programmatic manipulation of markdown content through
//! a Lisp-like representation.  The crate is designed for building knowledge bases
//! where documents need to be queried, transformed, and curated by automated agents.
//!
//! # Core Concepts
//!
//! ## S-expression Representation
//!
//! Markdown documents are represented as tagged S-expressions:
//!
//! ```text
//! (doc
//!   (h1 "Introduction")
//!   (p "Some text with " (em "emphasis") " and " (strong "bold") ".")
//!   (ul
//!     (li (p "First item"))
//!     (li (p "Second item"))))
//! ```
//!
//! ## Path-based Navigation
//!
//! Nodes are addressed using [`PathId`], a dot-separated path from the document root:
//! - `root` - the document itself
//! - `1` - first child (after the doc tag)
//! - `1.2` - second child of the first child
//!
//! ## Content-addressable References
//!
//! Nodes can also be referenced by [`ContentId`], a hash of their content,
//! enabling stable references that survive structural changes.
//!
//! # Example
//!
//! ```rust
//! use agentkb::{markdown_to_sexpr, sexpr_to_markdown, Parser, eval, Env, register_builtins};
//!
//! // Parse markdown to S-expression
//! let doc = markdown_to_sexpr("# Hello\n\nWorld!").unwrap();
//! assert!(doc.to_string().contains("(h1 \"Hello\")"));
//!
//! // Convert back to markdown
//! let md = sexpr_to_markdown(&doc).unwrap();
//! assert!(md.contains("# Hello"));
//! ```
//!
//! # Modules
//!
//! The crate re-exports types from internal modules:
//!
//! - **Core S-expression types**: [`SExpr`], [`Parser`], [`SError`], [`SResult`]
//! - **Evaluation**: [`Env`], [`eval`], [`register_builtins`], and built-in functions
//! - **JSON conversion**: [`json_to_sexpr`], [`sexpr_to_json`]
//! - **Markdown conversion**: [`markdown_to_sexpr`], [`sexpr_to_markdown`]
//! - **Node navigation**: [`PathId`], [`ContentId`], [`NodeId`], [`get_by_path`]
//! - **Mutations**: [`replace_at`], [`prune`], [`insert_before`], [`graft`]
//! - **Selectors**: [`Selector`], [`query`], [`select`]
//! - **Curation**: [`generate_toc`], [`wrap_in_callout`], [`mark_deprecated`]
//! - **Invariants**: [`Invariant`], [`assert_invariant`], [`validate_all`]
//! - **REPL**: [`Repl`], [`register_markdown_builtins`]

#![deny(missing_docs)]

pub mod agent;
pub mod chunker;
mod dialect;
mod s;

pub use dialect::{assoc, dissoc, get, keys, merge, values};
pub use s::error::{SError, SResult};
pub use s::eval::{
    Env, SExprFn, builtin_append, builtin_atom_p, builtin_cons, builtin_empty_p, builtin_eq_p,
    builtin_first, builtin_length, builtin_list, builtin_list_p, builtin_nth, builtin_null_p,
    builtin_rest, eval, register_builtins,
};
pub use s::expr::{Parser, SExpr};
pub use s::json::{
    json_to_sexpr, json_value_to_sexpr, sexpr_to_json, sexpr_to_json_value, unescape_string,
};
pub use s::markdown::curation::{
    ExtractResult, ExtractStrategy, LinkDefinition, LinkInfo, SkeletonSummary, TagInfo,
    extract_to_ref, extract_with_strategy, find_undefined_references, focus_context, generate_toc,
    get_external_links, get_image_links, get_internal_links, get_tagged_nodes, link_info_to_sexpr,
    mark_deprecated, merge_sections, normalize_headers, rehome_orphans, remove_tag,
    scan_link_definitions, scan_links, scan_links_to_sexpr, skeleton_summary,
    skeleton_summary_to_sexpr, skeletonize, tag_node, update_link, wrap_in_callout,
    wrap_in_details,
};
pub use s::markdown::invariants::{
    Invariant, ValidationResult, Violation, all_pass, assert_invariant, validate_all,
};
pub use s::markdown::mutations::{
    append_child, append_child_lenient, apply_mutations, apply_mutations_lenient, graft,
    graft_lenient, hoist, hoist_lenient, insert_after, insert_after_lenient, insert_before,
    insert_before_lenient, prepend_child, prepend_child_lenient, prune, prune_lenient, replace_at,
    replace_at_lenient,
};
pub use s::markdown::test_runner::{
    ExpectedError, Metadata, Mutation, TestCase, TestResult, TestSuite, run_test_directory,
    run_test_file,
};
pub use s::markdown::{
    get_frontmatter, get_frontmatter_content, get_frontmatter_field, markdown_to_sexpr,
    parse_yaml_frontmatter, remove_frontmatter, remove_frontmatter_field, set_frontmatter,
    sexpr_to_markdown, upsert_frontmatter_field,
};
pub use s::nodeid::{
    AnnotatedNode, ContentId, NodeId, PathId, annotate_document, get_by_content, get_by_path,
    get_context, get_node, get_parent, get_siblings, to_annotated_sexpr,
};
pub use s::repl::{Repl, register_markdown_builtins};
pub use s::selector::{
    AttributePredicate, Combinator, CompareOp, Selector, SelectorPart, SimpleSelector, query,
    query_one, select,
};
