# insert-before

Insert a new node before the node at a specific path.

## Syntax

```lisp
(insert-before doc path new-node)
```

## Description

`insert-before` returns a new document with a new node inserted immediately
before the node at the specified path.  Existing nodes shift to accommodate
the insertion.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying the reference node.
- **new-node**: The s-expression to insert.

## Returns

A new document with the node inserted.

## Examples

```lisp
;; Insert before paragraph
(insert-before
  (markdown-to-sexpr "# Title\n\nContent")
  "2"
  (quote (p "New paragraph before content.")))
;; => (doc (h1 "Title") (p "New...") (p "Content"))

;; Insert warning before section
(insert-before doc "3"
  (quote (blockquote (p "**Note:** Important information"))))

;; Insert at beginning
(insert-before doc "1"
  (quote (p "Preamble")))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `path-not-found` | Path doesn't exist in document |

## See Also

- [`insert-after`](insert-after.md) - Insert after instead
- [`prepend-child`](prepend-child.md) - Insert as first child
- [`replace-at`](replace-at.md) - Replace existing node
