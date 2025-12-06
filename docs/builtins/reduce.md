# reduce

Fold a list into a single value by applying a binary function cumulatively.

## Syntax

```lisp
(reduce function-name initial list-expr)
```

## Description

`reduce` (also known as fold-left) accumulates a result by repeatedly applying
a binary function to an accumulator and each successive element of a list.
Starting with the initial value, the function is called with `(func accumulator
element)` for each element, and the result becomes the new accumulator.

The function must be specified by name (as an atom).  It receives two arguments:
the current accumulated value and the next element.

## Arguments

- **function-name**: The name of a binary function (as an unquoted atom).
- **initial**: The starting value for the accumulator.
- **list-expr**: An expression that evaluates to a list.

## Returns

The final accumulated value after processing all elements.

## Examples

```lisp
;; Sum a list using +
(reduce + 0 (quote (1 2 3 4)))
;; 0 + 1 = 1
;; 1 + 2 = 3
;; 3 + 3 = 6
;; 6 + 4 = 10
;; => 10

;; Product using *
(reduce * 1 (quote (2 3 4)))
;; 1 * 2 * 3 * 4 = 24
;; => 24

;; Empty list returns initial value
(reduce + 42 (quote ()))
;; => 42

;; Flatten a list of lists using append
(reduce append (quote ()) (quote ((a) (b) (c))))
;; () + (a) = (a)
;; (a) + (b) = (a b)
;; (a b) + (c) = (a b c)
;; => (a b c)

;; Building a result from right to left with cons
(reduce cons (quote ()) (quote (a b c)))
;; (cons () a) => (a)  -- note: this isn't standard cons behavior
;; For proper reverse, you'd need a different approach
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `invalid-function` | First argument is not a function name |
| `type-error` | Third argument is not a list |
| `function-not-found` | Named function doesn't exist in environment |

## Notes

The order of arguments to the folding function is `(func accumulator element)`,
which is the fold-left convention.  The accumulator is always the first argument.

## See Also

- [`map`](map.md) - Transform each element
- [`filter`](filter.md) - Select elements
- [`append`](append.md) - Useful for flattening
- [`->>`](thread-last.md) - Build transformation pipelines
