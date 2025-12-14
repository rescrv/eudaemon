# merge

Combine multiple JSON objects into one, with later values overriding earlier.

## Syntax

```lisp
(merge object1 object2 ...)
```

## Description

`merge` combines multiple JSON objects into a single object.  Keys from later
objects override keys from earlier objects.  Non-object arguments are ignored.
With no arguments, returns an empty object.

This is a shallow merge—nested objects are not recursively merged.

## Arguments

- **object1, object2, ...**: Zero or more JSON objects to merge.

## Returns

A new object containing all key-value pairs from the input objects.

## Examples

```lisp
;; Merge two objects
(merge (obj ("a" 1) ("b" 2)) (obj ("c" 3)))
;; => (obj ("a" 1) ("b" 2) ("c" 3))

;; Later values override
(merge (obj ("x" 1)) (obj ("x" 2)) (obj ("x" 3)))
;; => (obj ("x" 3))

;; Partial override
(merge (obj ("a" 1) ("b" 2)) (obj ("b" 3) ("c" 4)))
;; => (obj ("a" 1) ("b" 3) ("c" 4))

;; Empty merge
(merge)
;; => (obj)

;; Single object
(merge (obj ("a" 1)))
;; => (obj ("a" 1))

;; Non-objects are ignored
(merge (obj ("a" 1)) "not-object" (obj ("b" 2)))
;; => (obj ("a" 1) ("b" 2))

;; Merge three objects
(merge
  (obj ("defaults" #t))
  (obj ("config" "base"))
  (obj ("override" "final")))
;; => (obj ("defaults" #t) ("config" "base") ("override" "final"))
```

## Notes

- This is a shallow merge; nested objects are replaced entirely, not merged
- Non-object arguments are silently skipped
- Key order follows insertion order: keys from earlier objects appear first,
  unless overridden

## See Also

- [`assoc`](assoc.md) - Add single key-value pair
- [`dissoc`](dissoc.md) - Remove keys before merging
