# splat

Refactor a document into a directory hierarchy based on header structure.

## Syntax

```lisp
(splat doc prefix)
```

## Description

`splat` takes a markdown document and explodes it into a directory tree based on
its header hierarchy. Each top-level section becomes a directory (or file),
with nested sections creating subdirectories.

For sections with children:
- Creates `prefix/slug/index.md` containing the section title, any direct content,
  and a table of contents linking to child sections

For leaf sections (no children):
- Creates `prefix/slug.md` containing the section content

This is useful for refactoring monolithic documentation into navigable
directory structures, or for breaking up large documents into manageable pieces.

## Arguments

- **doc**: An s-expression document to splat.
- **prefix**: A string path prefix for all generated files (e.g., `"output/"` or `""`).

## Returns

A list of file paths that were written.

## Examples

```lisp
;; Simple document with one section
(splat (load "intro.md") "docs/")
;; => ("docs/introduction.md")

;; Document with nested sections
;; Given: # Guide > ## Setup > ## Usage
(splat (load "guide.md") "")
;; => ("guide/index.md" "guide/setup.md" "guide/usage.md")

;; Deeply nested structure
;; Given: # Book > ## Part One > ### Chapter 1 > ### Chapter 2
(splat (load "book.md") "output/")
;; => ("output/book/index.md"
;;     "output/book/part-one/index.md"
;;     "output/book/part-one/chapter-1.md"
;;     "output/book/part-one/chapter-2.md")

;; Transform and splat
(splat
  (-> (load "monolith.md")
      (normalize-headers '((2 1) (3 2) (4 3))))
  "refactored/")
```

## Generated Index Files

When a section has children, `splat` creates an index file containing:

1. The section title as an h1 header
2. Any direct content from the section (paragraphs, etc.)
3. A bulleted list of links to child sections

For example, given:

```markdown
# Guide

Welcome to the guide.

## Getting Started

First steps...

## Advanced Topics

Deep dive...
```

The generated `guide/index.md` would be:

```markdown
# Guide

Welcome to the guide.

- [Getting Started](getting-started.md)
- [Advanced Topics](advanced-topics.md)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `no-filesystem` | No filesystem attached to VM |
| `io-error` | Error writing a file |
| `parent-dir-not-allowed` | Prefix contains `..` |

## See Also

- [`extract-sections`](../markdown/extract-sections.md) - Parse document into section hierarchy
- [`section-to-doc`](../markdown/section-to-doc.md) - Convert a section back to a document
- [`save`](save.md) - Save a single document
- [`slugify`](../markdown/slugify.md) - Convert text to URL-friendly slug
