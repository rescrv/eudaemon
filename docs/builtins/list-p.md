# list?

Test whether a value is a list.

## Syntax

```lisp
(list? expr)
```

## Description

`list?` is a predicate that returns `#t` if its argument is a list (including
the empty list), and `#f` if it is an atom.  Every s-expression is either an
atom or a list; this predicate distinguishes between them.

## Arguments

- **expr**: Any s-expression to test.

## Returns

- `#t` if the argument is a list
- `#f` if the argument is an atom

## Examples

```lisp
;; Testing lists
(list? (quote (a b c)))
;; => #t

(list? (quote ()))
;; => #t

;; Atoms are not lists
(list? foo)
;; => #f

(list? 42)
;; => #f

(list? null)
;; => #f

;; Useful for type dispatch
(if (list? x)
    (first x)
    x)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`atom?`](atom-p.md) - Complementary predicate
- [`empty?`](empty-p.md) - Test for empty list specifically
- [`filter`](filter.md) - Filter by predicate
