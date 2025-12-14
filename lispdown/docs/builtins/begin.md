# begin

Sequence multiple expressions, returning the result of the last.

## Syntax

```lisp
(begin expr1 expr2 ...)
```

## Description

`begin` is a special form that evaluates a sequence of expressions in order,
returning the result of the final expression.  It exists to group multiple
side-effecting operations or to execute several expressions where only one is
syntactically allowed.

Unlike `let`, `begin` does not introduce any variable bindings—it purely
sequences evaluation.

## Arguments

- **expr1, expr2, ...**: One or more expressions to evaluate in order.

## Returns

The result of the last expression.

## Examples

```lisp
;; Simple sequence
(begin 1 2 3)
;; => 3

;; Each expression is evaluated
(begin (+ 1 2) (+ 3 4) (+ 5 6))
;; => 11

;; Single expression
(begin (+ 1 2))
;; => 3

;; Useful in conditionals for multi-step branches
(if #t
    (begin
      (first (quote (a b c)))  ;; evaluated but discarded
      (rest (quote (a b c))))  ;; returned
    "else")
;; => (b c)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | No expressions provided |

## See Also

- [`let`](let.md) - Sequence with variable bindings
- [`->>`](thread-last.md) - Threading macro for pipelines
