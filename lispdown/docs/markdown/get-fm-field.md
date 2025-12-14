# get-fm-field

Get a field value from parsed frontmatter.

## Syntax

```lisp
(get-fm-field frontmatter-obj key)
```

## Description

`get-fm-field` retrieves a field value from a parsed frontmatter object.  This
is a convenience function for accessing frontmatter fields after parsing with
`parse-yaml-frontmatter`.

## Arguments

- **frontmatter-obj**: A parsed frontmatter object from `parse-yaml-frontmatter`.
- **key**: The field name to retrieve.

## Returns

- The field value as a string if present
- `null` if the field does not exist

## Examples

```lisp
;; Get a field
(get-fm-field
  (parse-yaml-frontmatter "title: Hello\nauthor: Alice")
  "title")
;; => "Hello"

;; Missing field returns null
(get-fm-field
  (parse-yaml-frontmatter "title: Hello")
  "missing")
;; => null

;; From document pipeline
(->> doc
     (get-frontmatter-content)
     (parse-yaml-frontmatter)
     (get-fm-field "status"))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |

## See Also

- [`parse-yaml-frontmatter`](parse-yaml-frontmatter.md) - Parse YAML first
- [`upsert-fm-field`](upsert-fm-field.md) - Update a field
