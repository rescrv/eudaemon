# keys

Extract the keys from a JSON object or indices from an array.

## Syntax

```lisp
(keys value)
```

## Description

`keys` returns an array of keys for a JSON object, or an array of integer
indices for a JSON array.  For objects, keys are returned in their original
order.  For arrays, indices are returned as `(arr 0 1 2 ...)`.

Non-object/array values return `null`.

## Arguments

- **value**: A JSON object or array s-expression.

## Returns

- For objects: `(arr "key1" "key2" ...)` with string keys
- For arrays: `(arr 0 1 2 ...)` with integer indices
- For other values: `null`

## Examples

```lisp
;; Keys of an object
(keys (obj ("a" 1) ("b" 2) ("c" 3)))
;; => (arr "a" "b" "c")

;; Keys preserve order
(keys (obj ("z" 1) ("a" 2) ("m" 3)))
;; => (arr "z" "a" "m")

;; Keys of empty object
(keys (obj))
;; => (arr)

;; Indices of an array
(keys (arr "x" "y" "z"))
;; => (arr 0 1 2)

;; Keys of non-object returns null
(keys "not-an-object")
;; => null
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`values`](values.md) - Extract values instead of keys
- [`get`](get.md) - Access by key
