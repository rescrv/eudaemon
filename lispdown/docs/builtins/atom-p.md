# atom?

Test whether a value is an atom.

## Syntax

```lisp
(atom? expr)
```

## Description

`atom?` is a predicate that returns `#t` if its argument is an atom, and `#f`
if it is a list.  Atoms are indivisible values: symbols, numbers, strings, and
special values like `null`, `#t`, and `#f`.  This predicate is the complement
of `list?`.

## Arguments

- **expr**: Any s-expression to test.

## Returns

- `#t` if the argument is an atom
- `#f` if the argument is a list

## Examples

```lisp
;; Testing atoms
(atom? foo)
;; => #t

(atom? 42)
;; => #t

(atom? null)
;; => #t

(atom? #t)
;; => #t

;; Lists are not atoms
(atom? (quote (a b c)))
;; => #f

(atom? (quote ()))
;; => #f

;; Useful for recursive processing
(if (atom? x)
    x  ;; base case
    (process-list x))  ;; recursive case
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`list?`](list-p.md) - Complementary predicate
- [`null?`](null-p.md) - Test for null specifically
