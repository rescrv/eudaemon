# append

Concatenate multiple lists into a single list.

## Syntax

```lisp
(append list1 list2 ...)
```

## Description

`append` concatenates any number of lists into a single flat list.  The
elements of each argument list appear in order in the result.  All arguments
must be lists.

Calling `append` with no arguments returns an empty list.  Calling it with a
single argument returns that list unchanged.

## Arguments

- **list1, list2, ...**: Zero or more lists to concatenate.

## Returns

A new list containing all elements from all argument lists, in order.

## Examples

```lisp
;; Append two lists
(append (quote (a b)) (quote (c d)))
;; => (a b c d)

;; Append multiple lists
(append (quote (a)) (quote (b)) (quote (c)))
;; => (a b c)

;; Append with empty list
(append (quote (a b)) (quote ()))
;; => (a b)

(append (quote ()) (quote (a b)))
;; => (a b)

;; No arguments yields empty list
(append)
;; => ()

;; Flattening with reduce
(reduce append (quote ()) (quote ((a) (b) (c))))
;; => (a b c)

;; Note: append is shallow, doesn't flatten nested lists
(append (quote ((a b))) (quote ((c d))))
;; => ((a b) (c d))
```

## Errors

| Code | Condition |
|------|-----------|
| `type-error` | Any argument is not a list |

## See Also

- [`cons`](cons.md) - Prepend single element
- [`list`](list.md) - Construct list from elements
- [`reduce`](reduce.md) - Flatten with `(reduce append ...)`
