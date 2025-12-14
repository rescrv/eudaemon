# length

Return the number of elements in a list.

## Syntax

```lisp
(length list-expr)
```

## Description

`length` returns the count of top-level elements in a list.  Nested lists
count as single elements; `length` does not recurse into sublists.

## Arguments

- **list-expr**: A list whose length to compute.

## Returns

An atom containing the integer count of elements.

## Examples

```lisp
;; Length of a list
(length (quote (a b c d)))
;; => 4

;; Empty list has length zero
(length (quote ()))
;; => 0

;; Single element
(length (quote (only)))
;; => 1

;; Nested lists count as one element each
(length (quote ((a b) (c d) (e f))))
;; => 3

;; After filtering
(->> (quote (1 2 3 4 5 6))
     (filter is-even)
     (length))
;; => 3
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |
| `type-error` | Argument is not a list |

## See Also

- [`nth`](nth.md) - Access element by index
- [`empty?`](empty-p.md) - Test if length is zero
