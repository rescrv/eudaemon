# arr

Construct a JSON array from elements.

## Syntax

```lisp
(arr element1 element2 ...)
```

## Description

`arr` constructs a JSON array s-expression from its arguments.  Each argument
becomes an element of the array in order.  This is the canonical representation
for JSON arrays in the s-expression dialect.

The `arr` tag is used both as a constructor and as a marker in parsed JSON
data.  When JSON is converted to s-expressions, arrays become `(arr ...)`
forms.

## Arguments

- **elements**: Zero or more values to include in the array.

## Returns

An s-expression of the form `(arr element1 element2 ...)`.

## Examples

```lisp
;; Empty array
(arr)
;; => (arr)

;; Array of numbers
(arr 1 2 3)
;; => (arr 1 2 3)

;; Array of strings
(arr "a" "b" "c")
;; => (arr "a" "b" "c")

;; Mixed types
(arr 1 "two" #t null)
;; => (arr 1 "two" #t null)

;; Nested arrays
(arr (arr 1 2) (arr 3 4))
;; => (arr (arr 1 2) (arr 3 4))

;; Array of objects
(arr (obj ("id" 1)) (obj ("id" 2)))
;; => (arr (obj ("id" 1)) (obj ("id" 2)))
```

## See Also

- [`obj`](obj.md) - Construct JSON objects
- [`keys`](keys.md) - Get array indices
- [`values`](values.md) - Get array elements
