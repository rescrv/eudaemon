# ->> (thread-last)

Thread an expression through a series of function calls as the last argument.

## Syntax

```lisp
(->> initial-expr form1 form2 ...)
```

## Description

`->>` (thread-last) is a macro that transforms nested function calls into a
linear pipeline.  The initial expression is evaluated, then its result is
inserted as the last argument of each subsequent form.  This is particularly
useful for data transformation pipelines where each step consumes and produces
a collection.

Each form can be either:
- A bare function name (e.g., `first`), which becomes `(func result)`
- A partial call (e.g., `(take 5)`), which becomes `(take 5 result)`

The result of each step becomes the input to the next.

## Arguments

- **initial-expr**: The starting value for the pipeline.
- **form1, form2, ...**: Function calls or function names to thread through.

## Returns

The result of threading the initial value through all forms.

## Examples

```lisp
;; Basic threading with single-argument functions
(->> 5 double)
;; Expands to: (double 5)
;; => 10

;; Threading through multi-argument functions
(->> 5 (+ 3))
;; Expands to: (+ 3 5)
;; => 8

;; Chaining multiple transformations
(->> 5 (+ 3) (* 2))
;; Expands to: (* 2 (+ 3 5))
;; => 16

;; Data transformation pipeline
(->> (quote (1 2 3 4 5))
     (filter is-even)
     (map double))
;; First: filter keeps (2 4)
;; Then: map doubles to (4 8)
;; => (4 8)

;; Processing markdown
(->> "# Title"
     (markdown-to-sexpr)
     (annotate))

;; Combining with list operations
(->> (quote (1 2 3))
     (append (quote (0)))
     (length))
;; => 4
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | No initial value provided |
| `invalid-thread-form` | A form is neither a function name nor a function call |

## Notes

The thread-last macro is ideal for collection-processing pipelines where
functions expect the collection as their last argument.  For functions that
expect the primary argument first, consider structuring your functions
accordingly.

## See Also

- [`begin`](begin.md) - Sequence without threading
- [`map`](map.md) - Transform each element
- [`filter`](filter.md) - Select elements
- [`reduce`](reduce.md) - Fold to single value
