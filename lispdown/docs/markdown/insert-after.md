# insert-after

Insert a new node after the node at a specific path.

## Syntax

```lisp
(insert-after doc path new-node)
```

## Description

`insert-after` returns a new document with a new node inserted immediately
after the node at the specified path.  Existing nodes shift to accommodate
the insertion.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying the reference node.
- **new-node**: The s-expression to insert.

## Returns

A new document with the node inserted.

## Examples

```lisp
;; Insert after heading
(insert-after
  (markdown-to-sexpr "# Title\n\nContent")
  "1"
  (quote (p "Subtitle or description.")))
;; => (doc (h1 "Title") (p "Subtitle...") (p "Content"))

;; Add note after specific section
(insert-after doc "2"
  (quote (blockquote (p "See also: related topic"))))

;; Insert at end by targeting last node
(insert-after doc "5"
  (quote (hr)))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `path-not-found` | Path doesn't exist in document |

## See Also

- [`insert-before`](insert-before.md) - Insert before instead
- [`append-child`](append-child.md) - Insert as last child
