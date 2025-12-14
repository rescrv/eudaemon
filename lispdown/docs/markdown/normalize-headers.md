# normalize-headers

Remap heading levels throughout a document according to a mapping.

## Syntax

```lisp
(normalize-headers doc depth-map)
```

## Description

`normalize-headers` adjusts all heading levels in a document according to a
mapping.  Each entry in the depth map specifies `(from-level to-level)`.  This
is useful for standardizing heading structure across documents or when merging
documents with different conventions.

## Arguments

- **doc**: A markdown document s-expression.
- **depth-map**: A list of `(from to)` pairs, where `from` and `to` are heading levels 1-6.

## Returns

A new document with heading levels remapped.

## Examples

```lisp
;; Demote all h1 to h2, h2 to h3
(normalize-headers
  (markdown-to-sexpr "# Title\n\n## Section")
  (quote ((1 2) (2 3))))
;; => (doc (h2 "Title") (h3 "Section"))

;; Flatten to single level
(normalize-headers doc
  (quote ((1 2) (2 2) (3 2) (4 2))))
;; All headings become h2

;; Promote subsections
(normalize-headers doc
  (quote ((3 2) (4 3))))
;; h3 becomes h2, h4 becomes h3

;; No-op for unmapped levels
(normalize-headers doc (quote ((1 2))))
;; Only h1 changes, h2-h6 stay the same
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `invalid-depth` | A depth value is not 1-6 |

## See Also

- [`hoist`](hoist.md) - Change single heading level
