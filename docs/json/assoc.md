# assoc

Associate a key with a value in a JSON object, returning a new object.

## Syntax

```lisp
(assoc object key value)
```

## Description

`assoc` returns a new JSON object with the specified key set to the given
value.  If the key already exists, its value is replaced; otherwise, a new
key-value pair is added to the end.  The original object is unchanged.

If given `null` as the object, `assoc` creates a new object with the single
key-value pair.

## Arguments

- **object**: A JSON object s-expression, or `null`.
- **key**: A string key to set.
- **value**: The value to associate with the key.

## Returns

A new object with the key-value pair added or updated.

## Examples

```lisp
;; Add new key
(assoc (obj ("a" 1)) "b" 2)
;; => (obj ("a" 1) ("b" 2))

;; Update existing key
(assoc (obj ("a" 1) ("b" 2)) "a" 10)
;; => (obj ("a" 10) ("b" 2))

;; Create object from null
(assoc null "key" 42)
;; => (obj ("key" 42))

;; Add to empty object
(assoc (obj) "first" "value")
;; => (obj ("first" "value"))

;; Chain associations
(->> (obj)
     (assoc "a" 1)
     (assoc "b" 2)
     (assoc "c" 3))
;; => (obj ("a" 1) ("b" 2) ("c" 3))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `invalid-key-type` | Key is not a string atom |

## Notes

Calling `assoc` on a non-object (except `null`) returns `null`.

## See Also

- [`dissoc`](dissoc.md) - Remove a key
- [`get`](get.md) - Retrieve a value
- [`merge`](merge.md) - Combine multiple objects
