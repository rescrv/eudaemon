# empty?

Test whether a value is the empty list.

## Syntax

```lisp
(empty? expr)
```

## Description

`empty?` is a predicate that returns `#t` if its argument is the empty list
`()`, and `#f` otherwise.  This is the canonical way to detect the end of a
list during recursive processing.

Note that `empty?` tests specifically for the empty list—it returns `#f` for
atoms (including `null`) and for non-empty lists.

## Arguments

- **expr**: Any s-expression to test.

## Returns

- `#t` if the argument is the empty list `()`
- `#f` otherwise

## Examples

```lisp
;; Testing empty list
(empty? (quote ()))
;; => #t

;; Non-empty lists
(empty? (quote (a)))
;; => #f

(empty? (quote (a b c)))
;; => #f

;; Atoms are not empty lists
(empty? null)
;; => #f

(empty? foo)
;; => #f

;; Common pattern: recursive list processing
(if (empty? xs)
    base-case
    (process (first xs) (rest xs)))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`null?`](null-p.md) - Test for null atom (different from empty list)
- [`list?`](list-p.md) - Test for any list
- [`first`](first.md) - Get first element (fails on empty)
- [`rest`](rest.md) - Get tail (fails on empty)
