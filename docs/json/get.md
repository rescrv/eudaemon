# get

Retrieve a value from a JSON object by key.

## Syntax

```lisp
(get object key)
```

## Description

`get` retrieves the value associated with a key in a JSON object.  If the key
exists, its value is returned; otherwise, `null` is returned.  The function
works only on `obj`-tagged s-expressions.

Keys are matched after unescaping quoted strings, so `"name"` matches the key
`name` in the object.

## Arguments

- **object**: A JSON object s-expression `(obj ...)`.
- **key**: A string key to look up.

## Returns

The value associated with the key, or `null` if not found.

## Examples

```lisp
;; Get existing key
(get (obj ("name" "Alice") ("age" 30)) "name")
;; => "Alice"

;; Get missing key returns null
(get (obj ("name" "Alice")) "missing")
;; => null

;; Nested access
(let ((user (obj ("profile" (obj ("email" "a@b.com"))))))
  (get (get user "profile") "email"))
;; => "a@b.com"

;; Get from non-object returns null
(get (arr 1 2 3) "key")
;; => null

;; Get from atom returns null
(get "not-an-object" "key")
;; => null
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `invalid-key-type` | Key is not a string atom |

## See Also

- [`assoc`](assoc.md) - Add or update a key
- [`dissoc`](dissoc.md) - Remove a key
- [`keys`](keys.md) - List all keys
