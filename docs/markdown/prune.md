# prune

Remove a node at a specific path from the document.

## Syntax

```lisp
(prune doc path)
```

## Description

`prune` returns a new document with the node at the specified path removed.
The original document is unchanged.  Siblings after the removed node shift to
fill the gap.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying the node to remove.

## Returns

A new document with the node removed.

## Examples

```lisp
;; Remove a paragraph
(prune
  (markdown-to-sexpr "# Title\n\nPara 1\n\nPara 2")
  "2")
;; => (doc (h1 "Title") (p "Para 2"))

;; Remove from list
(prune
  (markdown-to-sexpr "- A\n- B\n- C")
  "1.2")
;; => (doc (ul (li "A") (li "C")))

;; Remove deprecated section
(->> doc
     (prune "3")  ;; Remove third element
     (sexpr-to-markdown))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `path-not-found` | Path doesn't exist in document |

## Notes

Pruning a node causes all subsequent siblings to shift their path indices.
For example, if you prune path "2", the former path "3" becomes the new "2".
When pruning multiple nodes, work from highest path to lowest to avoid
invalidating paths.

## See Also

- [`replace-at`](replace-at.md) - Replace instead of remove
- [`graft`](graft.md) - Move instead of remove
- [`annotate`](annotate.md) - Find paths before pruning
