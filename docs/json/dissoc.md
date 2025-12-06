# dissoc

Remove a key from a JSON object, returning a new object.

## Syntax

```lisp
(dissoc object key)
```

## Description

`dissoc` returns a new JSON object with the specified key removed.  If the key
does not exist, the object is returned unchanged.  The original object is not
modified.

## Arguments

- **object**: A JSON object s-expression.
- **key**: A string key to remove.

## Returns

A new object without the specified key.

## Examples

```lisp
;; Remove existing key
(dissoc (obj ("a" 1) ("b" 2)) "a")
;; => (obj ("b" 2))

;; Remove non-existent key (unchanged)
(dissoc (obj ("a" 1)) "missing")
;; => (obj ("a" 1))

;; Remove last key yields empty object
(dissoc (obj ("only" 1)) "only")
;; => (obj)

;; Dissoc from empty object
(dissoc (obj) "any")
;; => (obj)

;; Chain with assoc
(->> (obj ("a" 1) ("b" 2) ("c" 3))
     (dissoc "b")
     (assoc "d" 4))
;; => (obj ("a" 1) ("c" 3) ("d" 4))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `invalid-key-type` | Key is not a string atom |

## Notes

Calling `dissoc` on a non-object returns `null`.

## See Also

- [`assoc`](assoc.md) - Add or update a key
- [`get`](get.md) - Retrieve a value before removing
