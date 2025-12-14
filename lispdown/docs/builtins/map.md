# map

Apply a function to each element of a list, producing a new list of results.

## Syntax

```lisp
(map function-name list-expr)
```

## Description

`map` is a higher-order function that transforms a list by applying a function
to each element.  The function is applied element-wise, and the results are
collected into a new list of the same length.  The original list is unchanged.

The function must be specified by name (as an atom), not as a lambda or
expression.  Each element is passed to the function as its sole argument.

## Arguments

- **function-name**: The name of a function to apply (as an unquoted atom).
- **list-expr**: An expression that evaluates to a list.

## Returns

A new list containing the results of applying the function to each element.

## Examples

```lisp
;; Double each number (assuming a 'double' function)
(map double (quote (1 2 3)))
;; => (2 4 6)

;; Map over empty list yields empty list
(map double (quote ()))
;; => ()

;; Combined with filter for select-and-transform
(filter is-even (map double (quote (1 2 3))))
;; => (2 4 6)  ;; all are even after doubling

;; Using first on lists of lists
(map first (quote ((a b) (c d) (e f))))
;; => (a c e)

;; In a pipeline
(->> (quote (1 2 3 4))
     (map double)
     (filter is-even))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `invalid-function` | First argument is not a function name |
| `type-error` | Second argument is not a list |
| `function-not-found` | Named function doesn't exist in environment |

## Notes

Unlike some languages, `map` cannot currently use inline lambda expressions.
The function must be defined separately or be a builtin.

## See Also

- [`filter`](filter.md) - Select elements matching a predicate
- [`reduce`](reduce.md) - Fold a list to a single value
- [`->>`](thread-last.md) - Build transformation pipelines
- [`first`](first.md) - Often used with map for extracting fields
