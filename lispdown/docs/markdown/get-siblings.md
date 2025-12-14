# get-siblings

Get all sibling nodes of a node at a given path.

## Syntax

```lisp
(get-siblings doc path)
```

## Description

`get-siblings` returns a list of all siblings of the node at the specified
path, including the node itself.  Siblings are nodes that share the same
parent.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying the reference node.

## Returns

A list of sibling nodes (including the node at the path).

## Examples

```lisp
;; Get siblings of a list item
(get-siblings
  (markdown-to-sexpr "- A\n- B\n- C")
  "1.1")  ;; First item
;; => ((li "A") (li "B") (li "C"))

;; Top-level siblings
(get-siblings
  (markdown-to-sexpr "# Title\n\nPara 1\n\nPara 2")
  "2")  ;; First paragraph
;; => ((h1 "Title") (p "Para 1") (p "Para 2"))

;; Single sibling (itself)
(get-siblings
  (markdown-to-sexpr "# Only")
  "1")
;; => ((h1 "Only"))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |

## See Also

- [`get-parent`](get-parent.md) - Get the shared parent
- [`get-context`](get-context.md) - Get windowed context
