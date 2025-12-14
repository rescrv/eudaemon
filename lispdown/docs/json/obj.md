# obj

Construct a JSON object from key-value pairs.

## Syntax

```lisp
(obj ("key1" value1) ("key2" value2) ...)
```

## Description

`obj` constructs a JSON object s-expression from key-value pair arguments.
Each pair is a two-element list where the first element is a quoted string key
and the second is any value.  This is the canonical representation for JSON
objects in the s-expression dialect.

The `obj` tag is used both as a constructor and as a marker in parsed JSON
data.  When JSON is converted to s-expressions, objects become `(obj ...)`
forms.

## Arguments

- **pairs**: Zero or more `("key" value)` pairs.

## Returns

An s-expression of the form `(obj ("key1" value1) ...)`.

## Examples

```lisp
;; Empty object
(obj)
;; => (obj)

;; Simple object
(obj ("name" "Alice") ("age" 30))
;; => (obj ("name" "Alice") ("age" 30))

;; Nested object
(obj ("user" (obj ("id" 1) ("name" "Bob"))))
;; => (obj ("user" (obj ("id" 1) ("name" "Bob"))))

;; Object with array value
(obj ("tags" (arr "a" "b" "c")))
;; => (obj ("tags" (arr "a" "b" "c")))
```

## See Also

- [`arr`](arr.md) - Construct JSON arrays
- [`get`](get.md) - Access object fields
- [`assoc`](assoc.md) - Add or update fields
- [`dissoc`](dissoc.md) - Remove fields
- [`keys`](keys.md) - Extract object keys
- [`values`](values.md) - Extract object values
- [`merge`](merge.md) - Combine objects
