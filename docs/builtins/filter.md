# filter

Select elements from a list where a predicate returns truthy.

## Syntax

```lisp
(filter predicate-name list-expr)
```

## Description

`filter` is a higher-order function that produces a new list containing only
the elements for which a predicate function returns a truthy value.  Elements
are tested in order, and those passing the test appear in the result in their
original order.

The predicate must be specified by name (as an atom).  Each element is passed
to the predicate as its sole argument.  See [`if`](if.md) for the definition
of truthiness.

## Arguments

- **predicate-name**: The name of a predicate function (as an unquoted atom).
- **list-expr**: An expression that evaluates to a list.

## Returns

A new list containing only elements where the predicate returned truthy.

## Examples

```lisp
;; Keep even numbers (assuming an 'is-even' predicate)
(filter is-even (quote (1 2 3 4 5 6)))
;; => (2 4 6)

;; Filter on empty list yields empty list
(filter is-even (quote ()))
;; => ()

;; When nothing matches, result is empty
(filter is-even (quote (1 3 5)))
;; => ()

;; Combined with map
(->> (quote (1 2 3 4 5 6))
     (filter is-even)
     (map double))
;; => (4 8 12)

;; Using list? to keep only lists
(filter list? (quote (a (b c) d (e f))))
;; => ((b c) (e f))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `invalid-function` | First argument is not a function name |
| `type-error` | Second argument is not a list |
| `function-not-found` | Named function doesn't exist in environment |

## Notes

Unlike some languages, `filter` cannot currently use inline lambda expressions.
The predicate must be defined separately or be a builtin like `list?` or `atom?`.

## See Also

- [`map`](map.md) - Transform elements
- [`reduce`](reduce.md) - Fold a list to a single value
- [`->>`](thread-last.md) - Build transformation pipelines
- [`list?`](list-p.md) - Common predicate for filtering
- [`empty?`](empty-p.md) - Test if filter result is empty
