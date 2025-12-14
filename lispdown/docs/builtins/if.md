# if

Conditionally evaluate one of two branches based on a boolean test.

## Syntax

```lisp
(if condition then-expr else-expr)
```

## Description

`if` is a special form that evaluates `condition`, then evaluates and returns
either `then-expr` or `else-expr` depending on whether the condition is truthy.
Only one branch is evaluated—the other is never executed.

### Truthiness

The following values are considered false (falsy):
- `#f` - The boolean false literal
- `null` - The null value
- `()` - The empty list

Everything else is truthy, including:
- `0` - Zero is truthy
- `""` - Empty strings are truthy
- Non-empty lists

## Arguments

- **condition**: Expression to evaluate for truthiness.
- **then-expr**: Expression to evaluate if condition is truthy.
- **else-expr**: Expression to evaluate if condition is falsy.

## Returns

The result of evaluating either `then-expr` or `else-expr`.

## Examples

```lisp
;; Basic conditional with boolean literals
(if #t "yes" "no")
;; => "yes"

(if #f "yes" "no")
;; => "no"

;; null is falsy
(if null "truthy" "falsy")
;; => "falsy"

;; Numbers (including 0) are truthy
(if 0 "truthy" "falsy")
;; => "truthy"

;; Empty list is falsy
(if (quote ()) "truthy" "falsy")
;; => "falsy"

;; Non-empty list is truthy
(if (quote (x)) "truthy" "falsy")
;; => "truthy"

;; Nested conditionals
(if #t
    (if #f "inner-then" "inner-else")
    "outer-else")
;; => "inner-else"

;; Combined with let for complex logic
(let ((x 10) (y 5))
  (if #t (+ x y) (- x y)))
;; => 15
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments provided |

## See Also

- [`let`](let.md) - Bind variables for use in conditions
- [`eq?`](eq-p.md) - Test equality for use as condition
