//! Agent-accessible documentation for the agentkb Lisp dialect.
//!
//! This module provides compile-time embedded documentation files that can be overlaid
//! onto a user's knowledge base using virtual file systems.  An agent consulting these
//! docs will have access to the full Lisp reference without network access.
//!
//! # Organization
//!
//! Documentation is organized into three categories:
//!
//! - **Builtins**: Core Lisp primitives like `cons`, `first`, `rest`, `map`, and `filter`.
//! - **JSON**: Functions for working with JSON objects and arrays.
//! - **Markdown**: Functions for parsing, querying, and transforming markdown documents.
//!
//! # Usage
//!
//! Each documentation file is available as a `(&str, &str)` tuple of `(path, contents)`.
//! The [`ALL_DOCS`] constant provides an array of all documentation files for easy iteration.

// =============================================================================
// Builtins Documentation
// =============================================================================

/// Documentation for the `append` builtin function.
pub const BUILTINS_APPEND: (&str, &str) = (
    "docs/builtins/append.md",
    include_str!("../docs/builtins/append.md"),
);

/// Documentation for the `atom?` builtin predicate.
pub const BUILTINS_ATOM_P: (&str, &str) = (
    "docs/builtins/atom-p.md",
    include_str!("../docs/builtins/atom-p.md"),
);

/// Documentation for the `begin` builtin form.
pub const BUILTINS_BEGIN: (&str, &str) = (
    "docs/builtins/begin.md",
    include_str!("../docs/builtins/begin.md"),
);

/// Documentation for the `cons` builtin function.
pub const BUILTINS_CONS: (&str, &str) = (
    "docs/builtins/cons.md",
    include_str!("../docs/builtins/cons.md"),
);

/// Documentation for the `empty?` builtin predicate.
pub const BUILTINS_EMPTY_P: (&str, &str) = (
    "docs/builtins/empty-p.md",
    include_str!("../docs/builtins/empty-p.md"),
);

/// Documentation for the `eq?` builtin predicate.
pub const BUILTINS_EQ_P: (&str, &str) = (
    "docs/builtins/eq-p.md",
    include_str!("../docs/builtins/eq-p.md"),
);

/// Documentation for the `filter` builtin function.
pub const BUILTINS_FILTER: (&str, &str) = (
    "docs/builtins/filter.md",
    include_str!("../docs/builtins/filter.md"),
);

/// Documentation for the `first` builtin function.
pub const BUILTINS_FIRST: (&str, &str) = (
    "docs/builtins/first.md",
    include_str!("../docs/builtins/first.md"),
);

/// Documentation for the `if` builtin form.
pub const BUILTINS_IF: (&str, &str) = (
    "docs/builtins/if.md",
    include_str!("../docs/builtins/if.md"),
);

/// Documentation for the `length` builtin function.
pub const BUILTINS_LENGTH: (&str, &str) = (
    "docs/builtins/length.md",
    include_str!("../docs/builtins/length.md"),
);

/// Documentation for the `let` builtin form.
pub const BUILTINS_LET: (&str, &str) = (
    "docs/builtins/let.md",
    include_str!("../docs/builtins/let.md"),
);

/// Documentation for the `list?` builtin predicate.
pub const BUILTINS_LIST_P: (&str, &str) = (
    "docs/builtins/list-p.md",
    include_str!("../docs/builtins/list-p.md"),
);

/// Documentation for the `list` builtin function.
pub const BUILTINS_LIST: (&str, &str) = (
    "docs/builtins/list.md",
    include_str!("../docs/builtins/list.md"),
);

/// Documentation for the `map` builtin function.
pub const BUILTINS_MAP: (&str, &str) = (
    "docs/builtins/map.md",
    include_str!("../docs/builtins/map.md"),
);

/// Documentation for the `nth` builtin function.
pub const BUILTINS_NTH: (&str, &str) = (
    "docs/builtins/nth.md",
    include_str!("../docs/builtins/nth.md"),
);

/// Documentation for the `null?` builtin predicate.
pub const BUILTINS_NULL_P: (&str, &str) = (
    "docs/builtins/null-p.md",
    include_str!("../docs/builtins/null-p.md"),
);

/// Documentation for the `quote` builtin form.
pub const BUILTINS_QUOTE: (&str, &str) = (
    "docs/builtins/quote.md",
    include_str!("../docs/builtins/quote.md"),
);

/// Documentation for the `reduce` builtin function.
pub const BUILTINS_REDUCE: (&str, &str) = (
    "docs/builtins/reduce.md",
    include_str!("../docs/builtins/reduce.md"),
);

/// Documentation for the `rest` builtin function.
pub const BUILTINS_REST: (&str, &str) = (
    "docs/builtins/rest.md",
    include_str!("../docs/builtins/rest.md"),
);

/// Documentation for the `thread-first` builtin macro.
pub const BUILTINS_THREAD_FIRST: (&str, &str) = (
    "docs/builtins/thread-first.md",
    include_str!("../docs/builtins/thread-first.md"),
);

/// Documentation for the `thread-last` builtin macro.
pub const BUILTINS_THREAD_LAST: (&str, &str) = (
    "docs/builtins/thread-last.md",
    include_str!("../docs/builtins/thread-last.md"),
);

// =============================================================================
// JSON Documentation
// =============================================================================

/// Documentation for the `arr` JSON function.
pub const JSON_ARR: (&str, &str) = ("docs/json/arr.md", include_str!("../docs/json/arr.md"));

/// Documentation for the `assoc` JSON function.
pub const JSON_ASSOC: (&str, &str) = ("docs/json/assoc.md", include_str!("../docs/json/assoc.md"));

/// Documentation for the `dissoc` JSON function.
pub const JSON_DISSOC: (&str, &str) = (
    "docs/json/dissoc.md",
    include_str!("../docs/json/dissoc.md"),
);

/// Documentation for the `get` JSON function.
pub const JSON_GET: (&str, &str) = ("docs/json/get.md", include_str!("../docs/json/get.md"));

/// Documentation for the `keys` JSON function.
pub const JSON_KEYS: (&str, &str) = ("docs/json/keys.md", include_str!("../docs/json/keys.md"));

/// Documentation for the `merge` JSON function.
pub const JSON_MERGE: (&str, &str) = ("docs/json/merge.md", include_str!("../docs/json/merge.md"));

/// Documentation for the `obj` JSON function.
pub const JSON_OBJ: (&str, &str) = ("docs/json/obj.md", include_str!("../docs/json/obj.md"));

/// Documentation for the `values` JSON function.
pub const JSON_VALUES: (&str, &str) = (
    "docs/json/values.md",
    include_str!("../docs/json/values.md"),
);

// =============================================================================
// Markdown Documentation
// =============================================================================

/// Documentation for the `annotate` markdown function.
pub const MARKDOWN_ANNOTATE: (&str, &str) = (
    "docs/markdown/annotate.md",
    include_str!("../docs/markdown/annotate.md"),
);

/// Documentation for the `append-child` markdown function.
pub const MARKDOWN_APPEND_CHILD: (&str, &str) = (
    "docs/markdown/append-child.md",
    include_str!("../docs/markdown/append-child.md"),
);

/// Documentation for the `find-undef-refs` markdown function.
pub const MARKDOWN_FIND_UNDEF_REFS: (&str, &str) = (
    "docs/markdown/find-undef-refs.md",
    include_str!("../docs/markdown/find-undef-refs.md"),
);

/// Documentation for the `generate-toc` markdown function.
pub const MARKDOWN_GENERATE_TOC: (&str, &str) = (
    "docs/markdown/generate-toc.md",
    include_str!("../docs/markdown/generate-toc.md"),
);

/// Documentation for the `get-by-path` markdown function.
pub const MARKDOWN_GET_BY_PATH: (&str, &str) = (
    "docs/markdown/get-by-path.md",
    include_str!("../docs/markdown/get-by-path.md"),
);

/// Documentation for the `get-context` markdown function.
pub const MARKDOWN_GET_CONTEXT: (&str, &str) = (
    "docs/markdown/get-context.md",
    include_str!("../docs/markdown/get-context.md"),
);

/// Documentation for the `get-external-links` markdown function.
pub const MARKDOWN_GET_EXTERNAL_LINKS: (&str, &str) = (
    "docs/markdown/get-external-links.md",
    include_str!("../docs/markdown/get-external-links.md"),
);

/// Documentation for the `get-fm-field` markdown function.
pub const MARKDOWN_GET_FM_FIELD: (&str, &str) = (
    "docs/markdown/get-fm-field.md",
    include_str!("../docs/markdown/get-fm-field.md"),
);

/// Documentation for the `get-frontmatter-content` markdown function.
pub const MARKDOWN_GET_FRONTMATTER_CONTENT: (&str, &str) = (
    "docs/markdown/get-frontmatter-content.md",
    include_str!("../docs/markdown/get-frontmatter-content.md"),
);

/// Documentation for the `get-frontmatter` markdown function.
pub const MARKDOWN_GET_FRONTMATTER: (&str, &str) = (
    "docs/markdown/get-frontmatter.md",
    include_str!("../docs/markdown/get-frontmatter.md"),
);

/// Documentation for the `get-image-links` markdown function.
pub const MARKDOWN_GET_IMAGE_LINKS: (&str, &str) = (
    "docs/markdown/get-image-links.md",
    include_str!("../docs/markdown/get-image-links.md"),
);

/// Documentation for the `get-internal-links` markdown function.
pub const MARKDOWN_GET_INTERNAL_LINKS: (&str, &str) = (
    "docs/markdown/get-internal-links.md",
    include_str!("../docs/markdown/get-internal-links.md"),
);

/// Documentation for the `get-node` markdown function.
pub const MARKDOWN_GET_NODE: (&str, &str) = (
    "docs/markdown/get-node.md",
    include_str!("../docs/markdown/get-node.md"),
);

/// Documentation for the `get-parent` markdown function.
pub const MARKDOWN_GET_PARENT: (&str, &str) = (
    "docs/markdown/get-parent.md",
    include_str!("../docs/markdown/get-parent.md"),
);

/// Documentation for the `get-siblings` markdown function.
pub const MARKDOWN_GET_SIBLINGS: (&str, &str) = (
    "docs/markdown/get-siblings.md",
    include_str!("../docs/markdown/get-siblings.md"),
);

/// Documentation for the `graft` markdown function.
pub const MARKDOWN_GRAFT: (&str, &str) = (
    "docs/markdown/graft.md",
    include_str!("../docs/markdown/graft.md"),
);

/// Documentation for the `hoist` markdown function.
pub const MARKDOWN_HOIST: (&str, &str) = (
    "docs/markdown/hoist.md",
    include_str!("../docs/markdown/hoist.md"),
);

/// Documentation for the `insert-after` markdown function.
pub const MARKDOWN_INSERT_AFTER: (&str, &str) = (
    "docs/markdown/insert-after.md",
    include_str!("../docs/markdown/insert-after.md"),
);

/// Documentation for the `insert-before` markdown function.
pub const MARKDOWN_INSERT_BEFORE: (&str, &str) = (
    "docs/markdown/insert-before.md",
    include_str!("../docs/markdown/insert-before.md"),
);

/// Documentation for the `mark-deprecated` markdown function.
pub const MARKDOWN_MARK_DEPRECATED: (&str, &str) = (
    "docs/markdown/mark-deprecated.md",
    include_str!("../docs/markdown/mark-deprecated.md"),
);

/// Documentation for the `markdown-to-sexpr` markdown function.
pub const MARKDOWN_MARKDOWN_TO_SEXPR: (&str, &str) = (
    "docs/markdown/markdown-to-sexpr.md",
    include_str!("../docs/markdown/markdown-to-sexpr.md"),
);

/// Documentation for the `normalize-headers` markdown function.
pub const MARKDOWN_NORMALIZE_HEADERS: (&str, &str) = (
    "docs/markdown/normalize-headers.md",
    include_str!("../docs/markdown/normalize-headers.md"),
);

/// Documentation for the `parse-yaml-frontmatter` markdown function.
pub const MARKDOWN_PARSE_YAML_FRONTMATTER: (&str, &str) = (
    "docs/markdown/parse-yaml-frontmatter.md",
    include_str!("../docs/markdown/parse-yaml-frontmatter.md"),
);

/// Documentation for the `prepend-child` markdown function.
pub const MARKDOWN_PREPEND_CHILD: (&str, &str) = (
    "docs/markdown/prepend-child.md",
    include_str!("../docs/markdown/prepend-child.md"),
);

/// Documentation for the `prune` markdown function.
pub const MARKDOWN_PRUNE: (&str, &str) = (
    "docs/markdown/prune.md",
    include_str!("../docs/markdown/prune.md"),
);

/// Documentation for the `remove-fm-field` markdown function.
pub const MARKDOWN_REMOVE_FM_FIELD: (&str, &str) = (
    "docs/markdown/remove-fm-field.md",
    include_str!("../docs/markdown/remove-fm-field.md"),
);

/// Documentation for the `remove-frontmatter` markdown function.
pub const MARKDOWN_REMOVE_FRONTMATTER: (&str, &str) = (
    "docs/markdown/remove-frontmatter.md",
    include_str!("../docs/markdown/remove-frontmatter.md"),
);

/// Documentation for the `replace-at` markdown function.
pub const MARKDOWN_REPLACE_AT: (&str, &str) = (
    "docs/markdown/replace-at.md",
    include_str!("../docs/markdown/replace-at.md"),
);

/// Documentation for the `scan-link-defs` markdown function.
pub const MARKDOWN_SCAN_LINK_DEFS: (&str, &str) = (
    "docs/markdown/scan-link-defs.md",
    include_str!("../docs/markdown/scan-link-defs.md"),
);

/// Documentation for the `scan-links` markdown function.
pub const MARKDOWN_SCAN_LINKS: (&str, &str) = (
    "docs/markdown/scan-links.md",
    include_str!("../docs/markdown/scan-links.md"),
);

/// Documentation for the `set-frontmatter` markdown function.
pub const MARKDOWN_SET_FRONTMATTER: (&str, &str) = (
    "docs/markdown/set-frontmatter.md",
    include_str!("../docs/markdown/set-frontmatter.md"),
);

/// Documentation for the `sexpr-to-markdown` markdown function.
pub const MARKDOWN_SEXPR_TO_MARKDOWN: (&str, &str) = (
    "docs/markdown/sexpr-to-markdown.md",
    include_str!("../docs/markdown/sexpr-to-markdown.md"),
);

/// Documentation for the `update-link` markdown function.
pub const MARKDOWN_UPDATE_LINK: (&str, &str) = (
    "docs/markdown/update-link.md",
    include_str!("../docs/markdown/update-link.md"),
);

/// Documentation for the `upsert-fm-field` markdown function.
pub const MARKDOWN_UPSERT_FM_FIELD: (&str, &str) = (
    "docs/markdown/upsert-fm-field.md",
    include_str!("../docs/markdown/upsert-fm-field.md"),
);

/// Documentation for the `wrap-in-callout` markdown function.
pub const MARKDOWN_WRAP_IN_CALLOUT: (&str, &str) = (
    "docs/markdown/wrap-in-callout.md",
    include_str!("../docs/markdown/wrap-in-callout.md"),
);

/// Documentation for the `wrap-in-details` markdown function.
pub const MARKDOWN_WRAP_IN_DETAILS: (&str, &str) = (
    "docs/markdown/wrap-in-details.md",
    include_str!("../docs/markdown/wrap-in-details.md"),
);

// =============================================================================
// Aggregate Constants
// =============================================================================

/// All builtin documentation files as (path, contents) tuples.
pub const BUILTINS_DOCS: [(&str, &str); 21] = [
    BUILTINS_APPEND,
    BUILTINS_ATOM_P,
    BUILTINS_BEGIN,
    BUILTINS_CONS,
    BUILTINS_EMPTY_P,
    BUILTINS_EQ_P,
    BUILTINS_FILTER,
    BUILTINS_FIRST,
    BUILTINS_IF,
    BUILTINS_LENGTH,
    BUILTINS_LET,
    BUILTINS_LIST_P,
    BUILTINS_LIST,
    BUILTINS_MAP,
    BUILTINS_NTH,
    BUILTINS_NULL_P,
    BUILTINS_QUOTE,
    BUILTINS_REDUCE,
    BUILTINS_REST,
    BUILTINS_THREAD_FIRST,
    BUILTINS_THREAD_LAST,
];

/// All JSON documentation files as (path, contents) tuples.
pub const JSON_DOCS: [(&str, &str); 8] = [
    JSON_ARR,
    JSON_ASSOC,
    JSON_DISSOC,
    JSON_GET,
    JSON_KEYS,
    JSON_MERGE,
    JSON_OBJ,
    JSON_VALUES,
];

/// All markdown documentation files as (path, contents) tuples.
pub const MARKDOWN_DOCS: [(&str, &str); 36] = [
    MARKDOWN_ANNOTATE,
    MARKDOWN_APPEND_CHILD,
    MARKDOWN_FIND_UNDEF_REFS,
    MARKDOWN_GENERATE_TOC,
    MARKDOWN_GET_BY_PATH,
    MARKDOWN_GET_CONTEXT,
    MARKDOWN_GET_EXTERNAL_LINKS,
    MARKDOWN_GET_FM_FIELD,
    MARKDOWN_GET_FRONTMATTER_CONTENT,
    MARKDOWN_GET_FRONTMATTER,
    MARKDOWN_GET_IMAGE_LINKS,
    MARKDOWN_GET_INTERNAL_LINKS,
    MARKDOWN_GET_NODE,
    MARKDOWN_GET_PARENT,
    MARKDOWN_GET_SIBLINGS,
    MARKDOWN_GRAFT,
    MARKDOWN_HOIST,
    MARKDOWN_INSERT_AFTER,
    MARKDOWN_INSERT_BEFORE,
    MARKDOWN_MARK_DEPRECATED,
    MARKDOWN_MARKDOWN_TO_SEXPR,
    MARKDOWN_NORMALIZE_HEADERS,
    MARKDOWN_PARSE_YAML_FRONTMATTER,
    MARKDOWN_PREPEND_CHILD,
    MARKDOWN_PRUNE,
    MARKDOWN_REMOVE_FM_FIELD,
    MARKDOWN_REMOVE_FRONTMATTER,
    MARKDOWN_REPLACE_AT,
    MARKDOWN_SCAN_LINK_DEFS,
    MARKDOWN_SCAN_LINKS,
    MARKDOWN_SET_FRONTMATTER,
    MARKDOWN_SEXPR_TO_MARKDOWN,
    MARKDOWN_UPDATE_LINK,
    MARKDOWN_UPSERT_FM_FIELD,
    MARKDOWN_WRAP_IN_CALLOUT,
    MARKDOWN_WRAP_IN_DETAILS,
];

/// All documentation files as (path, contents) tuples.
///
/// This array contains every embedded documentation file, suitable for populating
/// a virtual file system overlay.  Paths are relative to the crate root.
pub const ALL_DOCS: [(&str, &str); 65] = [
    // Builtins (21)
    BUILTINS_APPEND,
    BUILTINS_ATOM_P,
    BUILTINS_BEGIN,
    BUILTINS_CONS,
    BUILTINS_EMPTY_P,
    BUILTINS_EQ_P,
    BUILTINS_FILTER,
    BUILTINS_FIRST,
    BUILTINS_IF,
    BUILTINS_LENGTH,
    BUILTINS_LET,
    BUILTINS_LIST_P,
    BUILTINS_LIST,
    BUILTINS_MAP,
    BUILTINS_NTH,
    BUILTINS_NULL_P,
    BUILTINS_QUOTE,
    BUILTINS_REDUCE,
    BUILTINS_REST,
    BUILTINS_THREAD_FIRST,
    BUILTINS_THREAD_LAST,
    // JSON (8)
    JSON_ARR,
    JSON_ASSOC,
    JSON_DISSOC,
    JSON_GET,
    JSON_KEYS,
    JSON_MERGE,
    JSON_OBJ,
    JSON_VALUES,
    // Markdown (36)
    MARKDOWN_ANNOTATE,
    MARKDOWN_APPEND_CHILD,
    MARKDOWN_FIND_UNDEF_REFS,
    MARKDOWN_GENERATE_TOC,
    MARKDOWN_GET_BY_PATH,
    MARKDOWN_GET_CONTEXT,
    MARKDOWN_GET_EXTERNAL_LINKS,
    MARKDOWN_GET_FM_FIELD,
    MARKDOWN_GET_FRONTMATTER_CONTENT,
    MARKDOWN_GET_FRONTMATTER,
    MARKDOWN_GET_IMAGE_LINKS,
    MARKDOWN_GET_INTERNAL_LINKS,
    MARKDOWN_GET_NODE,
    MARKDOWN_GET_PARENT,
    MARKDOWN_GET_SIBLINGS,
    MARKDOWN_GRAFT,
    MARKDOWN_HOIST,
    MARKDOWN_INSERT_AFTER,
    MARKDOWN_INSERT_BEFORE,
    MARKDOWN_MARK_DEPRECATED,
    MARKDOWN_MARKDOWN_TO_SEXPR,
    MARKDOWN_NORMALIZE_HEADERS,
    MARKDOWN_PARSE_YAML_FRONTMATTER,
    MARKDOWN_PREPEND_CHILD,
    MARKDOWN_PRUNE,
    MARKDOWN_REMOVE_FM_FIELD,
    MARKDOWN_REMOVE_FRONTMATTER,
    MARKDOWN_REPLACE_AT,
    MARKDOWN_SCAN_LINK_DEFS,
    MARKDOWN_SCAN_LINKS,
    MARKDOWN_SET_FRONTMATTER,
    MARKDOWN_SEXPR_TO_MARKDOWN,
    MARKDOWN_UPDATE_LINK,
    MARKDOWN_UPSERT_FM_FIELD,
    MARKDOWN_WRAP_IN_CALLOUT,
    MARKDOWN_WRAP_IN_DETAILS,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_docs_count_matches_sum_of_categories() {
        assert_eq!(
            ALL_DOCS.len(),
            BUILTINS_DOCS.len() + JSON_DOCS.len() + MARKDOWN_DOCS.len(),
            "ALL_DOCS should contain exactly the sum of all category arrays"
        );
    }

    #[test]
    fn all_docs_have_nonempty_content() {
        for (path, content) in ALL_DOCS {
            assert!(
                !content.is_empty(),
                "Documentation file should have content: {}",
                path
            );
        }
    }

    #[test]
    fn all_paths_are_valid_relative_paths() {
        for (path, _) in ALL_DOCS {
            assert!(
                path.starts_with("docs/"),
                "Path should start with 'docs/': {}",
                path
            );
            assert!(
                path.ends_with(".md"),
                "Path should end with '.md': {}",
                path
            );
        }
    }

    #[test]
    fn builtin_docs_paths_are_in_builtins_directory() {
        for (path, _) in BUILTINS_DOCS {
            assert!(
                path.starts_with("docs/builtins/"),
                "Builtin doc path should be in docs/builtins/: {}",
                path
            );
        }
    }

    #[test]
    fn json_docs_paths_are_in_json_directory() {
        for (path, _) in JSON_DOCS {
            assert!(
                path.starts_with("docs/json/"),
                "JSON doc path should be in docs/json/: {}",
                path
            );
        }
    }

    #[test]
    fn markdown_docs_paths_are_in_markdown_directory() {
        for (path, _) in MARKDOWN_DOCS {
            assert!(
                path.starts_with("docs/markdown/"),
                "Markdown doc path should be in docs/markdown/: {}",
                path
            );
        }
    }
}
