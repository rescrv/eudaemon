# annotate

Add path identifiers to all nodes in a document.

## Syntax

```lisp
(annotate doc)
```

## Description

`annotate` transforms a document by adding path annotations to every node.
Each node receives an `@` prefix showing its path in the tree.  This is useful
for exploring document structure and finding paths for mutations.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

A new document with path annotations on all nodes.

## Examples

```lisp
;; Annotate a simple document
(annotate (markdown-to-sexpr "# Title\n\nSome text."))
;; => (doc
;;      (@1 (h1 (@1.1 "Title")))
;;      (@2 (p (@2.1 "Some text."))))

;; Find paths for nested structures
(annotate (markdown-to-sexpr "- Item 1\n- Item 2"))
;; => (doc
;;      (@1 (ul
;;        (@1.1 (li (@1.1.1 "Item 1")))
;;        (@1.2 (li (@1.2.1 "Item 2"))))))

;; Use annotations to identify edit targets
(->> "# Hello\n\n- A\n- B"
     (markdown-to-sexpr)
     (annotate))
;; See paths, then use them in mutations:
;; (prune doc "1.2")  ;; Remove second list item
```

## Notes

The annotated form is for inspection only.  Use the original paths with
mutation functions like `replace-at`, `prune`, `insert-before`, etc.

Path annotations use the `@` prefix (e.g., `@1`, `@1.2`) to distinguish them
from regular content.  Children are numbered starting at 1, since index 0 in
the s-expression is the tag name.

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`get-by-path`](get-by-path.md) - Navigate using paths
- [`replace-at`](replace-at.md) - Modify at path
- [`prune`](prune.md) - Remove at path
