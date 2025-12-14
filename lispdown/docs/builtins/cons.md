# cons

Construct a list by prepending an element to an existing list.

## Syntax

```lisp
(cons element list-expr)
```

## Description

`cons` constructs a new list with `element` as the first item and `list-expr`
as the remaining items.  This is the fundamental list construction operation
in Lisp, inverse to the `first`/`rest` decomposition.

The second argument must be a list.  To create a list from scratch, cons onto
the empty list.

## Arguments

- **element**: Any s-expression to become the first element.
- **list-expr**: A list to which the element will be prepended.

## Returns

A new list with `element` followed by all elements of `list-expr`.

## Examples

```lisp
;; Prepend to a list
(cons a (quote (b c)))
;; => (a b c)

;; Cons onto empty list
(cons a (quote ()))
;; => (a)

;; Cons a list onto a list
(cons (quote (a b)) (quote (c d)))
;; => ((a b) c d)

;; Building lists with repeated cons
(cons a (cons b (cons c (quote ()))))
;; => (a b c)

;; Reconstruction pattern
(cons (first xs) (rest xs))
;; => xs  (for non-empty xs)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `type-error` | Second argument is not a list |

## See Also

- [`first`](first.md) - Extract first element (inverse)
- [`rest`](rest.md) - Extract remaining elements (inverse)
- [`list`](list.md) - Construct list from multiple elements
- [`append`](append.md) - Concatenate lists
