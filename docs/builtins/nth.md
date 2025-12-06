# nth

Return the element at a specific index in a list.

## Syntax

```lisp
(nth index list-expr)
```

## Description

`nth` returns the element at the specified zero-based index in a list.  The
index must be a non-negative integer less than the list's length.

This provides random access to list elements without repeated `first`/`rest`
decomposition.

## Arguments

- **index**: A non-negative integer (as an atom).
- **list-expr**: A list to index into.

## Returns

The element at the specified index.

## Examples

```lisp
;; First element (index 0)
(nth 0 (quote (a b c)))
;; => a

;; Second element (index 1)
(nth 1 (quote (a b c)))
;; => b

;; Last element of known-length list
(nth 2 (quote (a b c)))
;; => c

;; Accessing nested list element
(nth 1 (quote ((a b) (c d) (e f))))
;; => (c d)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `type-error` | Index is not an atom, or list-expr is not a list |
| `invalid-index` | Index is not a valid non-negative integer |
| `index-out-of-bounds` | Index >= length of list |

## See Also

- [`first`](first.md) - Equivalent to `(nth 0 ...)`
- [`length`](length.md) - Get list length for bounds checking
