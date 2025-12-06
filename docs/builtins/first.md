# first

Extract the first element of a list.

## Syntax

```lisp
(first list-expr)
```

## Description

`first` returns the first element of a list (traditionally called `car` in
Lisp).  The list must be non-empty; calling `first` on an empty list is an
error.

This is the fundamental list deconstruction operation, typically paired with
`rest` to process lists recursively.

## Arguments

- **list-expr**: A non-empty list.

## Returns

The first element of the list.

## Examples

```lisp
;; First of a list
(first (quote (a b c)))
;; => a

;; First of single-element list
(first (quote (only)))
;; => only

;; First of nested list
(first (quote ((a b) c d)))
;; => (a b)

;; Common recursive pattern
(if (empty? xs)
    ()
    (cons (process (first xs))
          (recur (rest xs))))

;; Combined with map
(map first (quote ((a 1) (b 2) (c 3))))
;; => (a b c)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |
| `type-error` | Argument is not a list |
| `empty-list` | List is empty |

## See Also

- [`rest`](rest.md) - Get all but first element
- [`nth`](nth.md) - Get element at specific index
- [`cons`](cons.md) - Prepend element to list
