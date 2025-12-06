# eq?

Test structural equality of two s-expressions.

## Syntax

```lisp
(eq? expr1 expr2)
```

## Description

`eq?` is a predicate that returns `#t` if its two arguments are structurally
equal, and `#f` otherwise.  Two atoms are equal if they have the same string
representation.  Two lists are equal if they have the same length and
corresponding elements are equal (recursive structural comparison).

## Arguments

- **expr1**: First s-expression to compare.
- **expr2**: Second s-expression to compare.

## Returns

- `#t` if the arguments are structurally equal
- `#f` otherwise

## Examples

```lisp
;; Equal atoms
(eq? foo foo)
;; => #t

(eq? 42 42)
;; => #t

;; Unequal atoms
(eq? foo bar)
;; => #f

;; Equal lists
(eq? (quote (1 2)) (quote (1 2)))
;; => #t

(eq? (quote (a (b c))) (quote (a (b c))))
;; => #t

;; Unequal lists
(eq? (quote (1 2)) (quote (1 3)))
;; => #f

(eq? (quote (1 2)) (quote (1 2 3)))
;; => #f

;; Atom vs list
(eq? foo (quote (foo)))
;; => #f

;; Empty lists are equal
(eq? (quote ()) (quote ()))
;; => #t
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |

## See Also

- [`if`](if.md) - Use equality in conditionals
- [`filter`](filter.md) - Filter by equality predicate
