# append-child

Append a child node to a parent node at a specific path.

## Syntax

```lisp
(append-child doc parent-path child-node)
```

## Description

`append-child` returns a new document with a child node appended to the end
of the children of the node at the specified path.  The parent node must be a
list-type node (like `doc`, `ul`, `ol`, `blockquote`).

## Arguments

- **doc**: A markdown document s-expression.
- **parent-path**: A path string identifying the parent node.
- **child-node**: The s-expression to append as a child.

## Returns

A new document with the child appended.

## Examples

```lisp
;; Append to document root
(append-child
  (markdown-to-sexpr "# Title")
  ""  ;; Empty path = root
  (quote (p "New paragraph at end.")))
;; => (doc (h1 "Title") (p "New paragraph at end."))

;; Append item to list
(append-child
  (markdown-to-sexpr "- A\n- B")
  "1"  ;; The ul element
  (quote (li "C")))
;; => (doc (ul (li "A") (li "B") (li "C")))

;; Add to blockquote
(append-child doc "2"  ;; Path to blockquote
  (quote (p "Additional quoted text.")))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `path-not-found` | Path doesn't exist in document |

## See Also

- [`prepend-child`](prepend-child.md) - Insert as first child
- [`insert-after`](insert-after.md) - Insert after sibling
