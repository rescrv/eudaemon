# get-context

Get surrounding nodes within a radius of a target node.

## Syntax

```lisp
(get-context doc path radius)
```

## Description

`get-context` returns a window of sibling nodes surrounding the node at the
specified path.  The radius determines how many siblings before and after the
target node are included.  This is useful for understanding the local context
of a node before making edits.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying the center node.
- **radius**: Number of siblings to include on each side.

## Returns

A list of nodes centered on the target, within the radius.

## Examples

```lisp
;; Get context with radius 1
(get-context
  (markdown-to-sexpr "# A\n\n# B\n\n# C\n\n# D\n\n# E")
  "3"  ;; Node C
  1)   ;; One before, one after
;; => ((h1 "B") (h1 "C") (h1 "D"))

;; Larger radius
(get-context doc "5" 2)
;; Returns nodes 3, 4, 5, 6, 7 (if they exist)

;; Radius extends to boundaries
(get-context
  (markdown-to-sexpr "# A\n\n# B")
  "1"  ;; First node
  5)   ;; Large radius
;; => ((h1 "A") (h1 "B"))  ;; Only available siblings
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `invalid-radius` | Radius is not a non-negative integer |

## See Also

- [`get-siblings`](get-siblings.md) - Get all siblings
- [`get-by-path`](get-by-path.md) - Get single node
