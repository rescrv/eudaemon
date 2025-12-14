# hoist

Change the level of a heading by a delta amount.

## Syntax

```lisp
(hoist doc path delta)
```

## Description

`hoist` promotes or demotes a heading by changing its level.  A negative delta
promotes the heading (e.g., h2 → h1), while a positive delta demotes it (e.g.,
h1 → h2).  The heading level is clamped to the valid range 1-6.

This is useful for reorganizing document structure when merging or extracting
sections.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying a heading node.
- **delta**: An integer to add to the heading level.

## Returns

A new document with the heading level adjusted.

## Examples

```lisp
;; Promote h2 to h1
(hoist
  (markdown-to-sexpr "## Section")
  "1"
  -1)
;; => (doc (h1 "Section"))

;; Demote h1 to h2
(hoist doc "1" 1)
;; h1 becomes h2

;; Demote by two levels
(hoist doc "1" 2)
;; h1 becomes h3

;; Level clamped at boundaries
(hoist (markdown-to-sexpr "# Top") "1" -5)
;; Still h1 (can't go below 1)

(hoist (markdown-to-sexpr "###### Deep") "1" 5)
;; Still h6 (can't go above 6)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `invalid-delta` | Delta is not an integer |
| `path-not-found` | Path doesn't exist in document |
| `not-a-heading` | Node at path is not a heading |

## Notes

This function only affects heading level, not heading content or structure.
To change the heading text, use `replace-at` with a new heading node.

The delta is applied directly: `h2 + delta(-1) = h1`, `h2 + delta(2) = h4`.

## See Also

- [`replace-at`](replace-at.md) - General node replacement
- [`normalize-headers`](normalize-headers.md) - Batch heading adjustments
- [`annotate`](annotate.md) - Find heading paths
