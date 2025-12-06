# prepend-child

Prepend a child node to a parent node at a specific path.

## Syntax

```lisp
(prepend-child doc parent-path child-node)
```

## Description

`prepend-child` returns a new document with a child node inserted at the
beginning of the children of the node at the specified path.  The parent node
must be a list-type node (like `doc`, `ul`, `ol`, `blockquote`).

## Arguments

- **doc**: A markdown document s-expression.
- **parent-path**: A path string identifying the parent node.
- **child-node**: The s-expression to prepend as a child.

## Returns

A new document with the child prepended.

## Examples

```lisp
;; Prepend to document root
(prepend-child
  (markdown-to-sexpr "# Title")
  ""  ;; Empty path = root
  (quote (p "Preamble before everything.")))
;; => (doc (p "Preamble...") (h1 "Title"))

;; Prepend item to list
(prepend-child
  (markdown-to-sexpr "- B\n- C")
  "1"  ;; The ul element
  (quote (li "A")))
;; => (doc (ul (li "A") (li "B") (li "C")))

;; Add warning to beginning of section
(prepend-child doc "3"  ;; Path to section container
  (quote (p "**Warning:** Read carefully.")))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `path-not-found` | Path doesn't exist in document |

## See Also

- [`append-child`](append-child.md) - Insert as last child
- [`insert-before`](insert-before.md) - Insert before sibling
