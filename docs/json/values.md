# values

Extract the values from a JSON object or elements from an array.

## Syntax

```lisp
(values value)
```

## Description

`values` returns an array containing all values from a JSON object, or the
array itself if given an array.  For objects, values are returned in the same
order as their corresponding keys.

Non-object/array values return `null`.

## Arguments

- **value**: A JSON object or array s-expression.

## Returns

- For objects: `(arr value1 value2 ...)` with the object's values
- For arrays: The array unchanged
- For other values: `null`

## Examples

```lisp
;; Values of an object
(values (obj ("a" 1) ("b" 2) ("c" 3)))
;; => (arr 1 2 3)

;; Values preserve key order
(values (obj ("z" 100) ("a" 200)))
;; => (arr 100 200)

;; Values of empty object
(values (obj))
;; => (arr)

;; Array returns itself
(values (arr 1 2 3))
;; => (arr 1 2 3)

;; Values of non-object returns null
(values "not-an-object")
;; => null
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`keys`](keys.md) - Extract keys instead of values
- [`get`](get.md) - Access specific value by key
