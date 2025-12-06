# graft

Move a subtree from one location to another within the document.

## Syntax

```lisp
(graft doc source-path target-parent-path target-index)
```

## Description

`graft` moves a node (and its children) from one location to another.  The
node is removed from its source location and inserted at the specified index
under the target parent.  This is an atomic move operation—the document is
consistent throughout.

## Arguments

- **doc**: A markdown document s-expression.
- **source-path**: Path to the node to move.
- **target-parent-path**: Path to the new parent node.
- **target-index**: Position among siblings (1-indexed for children).

## Returns

A new document with the node moved.

## Examples

```lisp
;; Move second section to first position
(graft doc "2" "" 1)
;; The node at path 2 becomes the first child of root

;; Move list item within list
(graft
  (markdown-to-sexpr "- A\n- B\n- C")
  "1.3"    ;; Source: third item (C)
  "1"      ;; Target parent: the ul
  1)       ;; Insert at position 1
;; => (doc (ul (li "C") (li "A") (li "B")))

;; Move section into blockquote
(graft doc "3" "2" 1)
;; Node at path 3 becomes first child of node at path 2
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly four arguments |
| `invalid-index` | Target index is not a non-negative integer |
| `path-not-found` | Source or target path doesn't exist |

## Notes

If the source is an ancestor of the target, the operation will fail to prevent
creating circular structures.

## See Also

- [`prune`](prune.md) - Remove without moving
- [`insert-before`](insert-before.md) - Insert at specific position
