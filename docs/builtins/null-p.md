# null?

Test whether a value is the null atom.

## Syntax

```lisp
(null? expr)
```

## Description

`null?` is a predicate that returns `#t` if its argument is the atom `null`,
and `#f` otherwise.  This is the only way to distinguish null from other falsy
values like `#f` or the empty list.

## Arguments

- **expr**: Any s-expression to test.

## Returns

- `#t` if the argument is the atom `null`
- `#f` otherwise

## Examples

```lisp
;; Testing null
(null? null)
;; => #t

;; Other atoms are not null
(null? foo)
;; => #f

(null? #f)
;; => #f

;; Lists are not null
(null? (quote ()))
;; => #f

(null? (quote (a b)))
;; => #f
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`empty?`](empty-p.md) - Test for empty list
- [`atom?`](atom-p.md) - Test for any atom
- [`if`](if.md) - Truthiness rules (null is falsy)
