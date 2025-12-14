# get-parent

Get the parent node of a node at a given path.

## Syntax

```lisp
(get-parent doc path)
```

## Description

`get-parent` returns the parent node of the node at the specified path.  For
root-level nodes (direct children of `doc`), the parent is the document itself.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying the child node.

## Returns

- The parent node if the path is valid
- `null` if the path is invalid or has no parent (is root)

## Examples

```lisp
;; Get parent of nested item
(get-parent
  (markdown-to-sexpr "- A\n- B")
  "1.1")  ;; Path to first list item
;; => (ul (li "A") (li "B"))

;; Parent of top-level element is doc
(get-parent
  (markdown-to-sexpr "# Title\n\nText")
  "1")
;; => (doc (h1 "Title") (p "Text"))

;; Invalid path returns null
(get-parent doc "99.99")
;; => null
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |

## See Also

- [`get-by-path`](get-by-path.md) - Get the node itself
- [`get-siblings`](get-siblings.md) - Get sibling nodes
- [`get-context`](get-context.md) - Get surrounding context
