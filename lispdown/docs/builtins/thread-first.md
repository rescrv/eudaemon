# -> (thread-first)

Thread an expression through a series of function calls as the first argument.

## Syntax

```lisp
(-> initial-expr form1 form2 ...)
```

## Description

`->` (thread-first) is a macro that transforms nested function calls into a
linear pipeline.  The initial expression is evaluated, then its result is
inserted as the first argument of each subsequent form.  This is particularly
useful for pipelines where each step operates on a primary subject that is
conventionally passed as the first argument.

Each form can be either:
- A bare function name (e.g., `first`), which becomes `(func result)`
- A partial call (e.g., `(get "key")`), which becomes `(get result "key")`

The result of each step becomes the input to the next.

## Arguments

- **initial-expr**: The starting value for the pipeline.
- **form1, form2, ...**: Function calls or function names to thread through.

## Returns

The result of threading the initial value through all forms.

## Examples

```lisp
;; Basic threading with single-argument functions
(-> 5 double)
;; Expands to: (double 5)
;; => 10

;; Threading through multi-argument functions
(-> 5 (+ 3))
;; Expands to: (+ 5 3)
;; => 8

;; Chaining multiple transformations
(-> 5 (+ 3) (* 2))
;; Expands to: (* (+ 5 3) 2)
;; => 16

;; Processing a document - thread-first is natural for object-centric operations
(-> doc
    (get-by-path "1")
    (annotate))

;; JSON object manipulation
(-> (obj)
    (assoc "name" "Alice")
    (assoc "age" 30))
;; => {"name": "Alice", "age": 30}
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | No initial value provided |
| `invalid-thread-form` | A form is neither a function name nor a function call |

## Notes

The thread-first macro is ideal for object-manipulation pipelines where
functions expect the primary subject as their first argument.  For
collection-processing pipelines where the collection is typically the last
argument, use [`->>`](thread-last.md) (thread-last) instead.

## See Also

- [`->>`](thread-last.md) - Thread as last argument (for collection pipelines)
- [`begin`](begin.md) - Sequence without threading
