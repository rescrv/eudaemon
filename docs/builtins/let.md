# let

Bind variables to values within a lexical scope.

## Syntax

```lisp
(let ((var1 val1) (var2 val2) ...) body-expr ...)
```

## Description

`let` is a special form that introduces local variable bindings.  Each binding
associates a name with a value, and those bindings are visible only within the
body expressions.  The body may contain multiple expressions; all are evaluated
in order and the result of the last expression is returned.

Bindings are evaluated in the parent environment, meaning earlier bindings are
not visible to later bindings within the same `let` form.  For sequential
bindings where later values depend on earlier ones, nest `let` forms.

Variable shadowing is supported: an inner `let` can rebind a name from an outer
scope, and the inner binding takes precedence within its scope.

## Arguments

- **bindings**: A list of `(variable value)` pairs.
- **body-expr**: One or more expressions to evaluate with the bindings in scope.

## Returns

The result of the last body expression.

## Examples

```lisp
;; Simple binding
(let ((x 5)) x)
;; => 5

;; Multiple bindings
(let ((x 5) (y 10)) (+ x y))
;; => 15

;; Computed values in bindings
(let ((x (+ 2 3))) x)
;; => 5

;; Multiple body expressions (returns last)
(let ((x 5))
  (+ x 1)
  (+ x 2))
;; => 7

;; Nested let for sequential bindings
(let ((x 5))
  (let ((y (+ x 5)))
    (+ x y)))
;; => 15

;; Variable shadowing
(let ((x 5))
  (let ((x 10))
    x))
;; => 10

;; After inner scope, outer binding still valid
(let ((x 5))
  (let ((y x))  ;; y = 5
    (+ x y)))
;; => 10
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Fewer than two arguments (bindings + body) |
| `invalid-let-bindings` | Bindings is not a list |
| `invalid-binding` | A binding is not a `(var value)` pair |
| `invalid-binding-name` | Variable name is not an atom |

## See Also

- [`begin`](begin.md) - Sequence expressions without bindings
- [`if`](if.md) - Conditional with let-bound variables
- [`->>`](thread-last.md) - Alternative for chained transformations
