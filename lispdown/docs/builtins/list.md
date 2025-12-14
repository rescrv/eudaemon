# list

Construct a list from evaluated arguments.

## Syntax

```lisp
(list expr1 expr2 ...)
```

## Description

`list` is a function that evaluates all its arguments and returns them as a
list.  Unlike `quote`, which prevents evaluation, `list` evaluates each
argument before collecting them.  With no arguments, it returns an empty list.

## Arguments

- **expr1, expr2, ...**: Zero or more expressions to evaluate and collect.

## Returns

A list containing the evaluated arguments.

## Examples

```lisp
;; Create list from atoms
(list a b c)
;; => (a b c)

;; Arguments are evaluated
(list (+ 1 2) (+ 3 4))
;; => (3 7)

;; Empty list
(list)
;; => ()

;; Single element
(list x)
;; => (x)

;; Contrast with quote
(quote (a b c))  ;; literal, unevaluated
(list a b c)     ;; a, b, c are evaluated as variables/atoms

;; Nested list construction
(list (list a b) (list c d))
;; => ((a b) (c d))

;; Mixed evaluated and literal content
(list 'head (+ 1 2) 'tail)
;; => (head 3 tail)
```

## Errors

This function does not produce errors from its own logic (any argument count
is valid), but evaluation of arguments may produce errors.

## See Also

- [`quote`](quote.md) - Literal data without evaluation
- [`cons`](cons.md) - Prepend single element
- [`append`](append.md) - Concatenate existing lists
