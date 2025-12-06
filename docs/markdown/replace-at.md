# replace-at

Replace a node at a specific path with a new node.

## Syntax

```lisp
(replace-at doc path new-node)
```

## Description

`replace-at` returns a new document with the node at the specified path
replaced by a new node.  The original document is unchanged.  This is the
fundamental mutation operation for in-place modification.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying the node to replace.
- **new-node**: The s-expression to put in place of the old node.

## Returns

A new document with the replacement made.

## Examples

```lisp
;; Replace a heading
(replace-at
  (markdown-to-sexpr "# Old Title\n\nContent")
  "1"
  (quote (h1 "New Title")))
;; => (doc (h1 "New Title") (p "Content"))

;; Replace paragraph text
(replace-at doc "2" (quote (p "Updated text.")))

;; Replace list item
(replace-at
  (markdown-to-sexpr "- A\n- B\n- C")
  "1.2"
  (quote (li "Modified B")))

;; Pipeline with replace
(->> "# Title"
     (markdown-to-sexpr)
     (replace-at "1" (quote (h2 "Demoted")))
     (sexpr-to-markdown))
;; => "## Demoted\n"
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `path-not-found` | Path doesn't exist in document |

## See Also

- [`prune`](prune.md) - Remove without replacement
- [`insert-before`](insert-before.md) - Add adjacent node
- [`hoist`](hoist.md) - Change heading level
