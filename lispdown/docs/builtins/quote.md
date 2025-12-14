# quote

Prevent evaluation of an expression, returning it as literal data.

## Syntax

```lisp
(quote expr)
'expr
```

## Description

`quote` is a special form that returns its argument unevaluated.  In Lisp
tradition, quoting transforms code into data—the expression becomes a value
rather than something to be executed.  The single-quote (`'`) character serves
as syntactic sugar for `(quote ...)`.

## Arguments

- **expr**: Any s-expression to be returned literally.

## Returns

The exact s-expression provided, without evaluation.

## Examples

```lisp
;; Quote an atom
(quote foo)
;; => foo

;; Quote a list - returns the list as data, not a function call
(quote (a b c))
;; => (a b c)

;; Syntactic sugar using single quote
'foo
;; => (quote foo)

'(a b c)
;; => (quote (a b c))

;; Useful for constructing literal data structures
(list 'a 'b 'c)
;; => (a b c)

;; Without quote, this would try to call function 'add'
(quote (add 1 2))
;; => (add 1 2)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | More or fewer than one argument provided |

## See Also

- [`list`](list.md) - Construct a list from evaluated arguments
