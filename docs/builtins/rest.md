# rest

Extract all but the first element of a list.

## Syntax

```lisp
(rest list-expr)
```

## Description

`rest` returns a list containing all elements except the first (traditionally
called `cdr` in Lisp).  The list must be non-empty; calling `rest` on an empty
list is an error.  The result may be empty if the input had exactly one element.

This is the fundamental list deconstruction operation, typically paired with
`first` to process lists recursively.

## Arguments

- **list-expr**: A non-empty list.

## Returns

A list containing all elements except the first.

## Examples

```lisp
;; Rest of a list
(rest (quote (a b c)))
;; => (b c)

;; Rest of two-element list
(rest (quote (a b)))
;; => (b)

;; Rest of single-element list is empty
(rest (quote (a)))
;; => ()

;; Common recursive pattern
(if (empty? xs)
    result
    (recur (process (first xs)) (rest xs)))

;; Repeated application
(rest (rest (rest (quote (a b c d e)))))
;; => (d e)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |
| `type-error` | Argument is not a list |
| `empty-list` | List is empty |

## See Also

- [`first`](first.md) - Get first element
- [`nth`](nth.md) - Get element at specific index
- [`cons`](cons.md) - Reconstruct list with element prepended
