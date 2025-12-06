# upsert-fm-field

Update or insert a field in YAML frontmatter text.

## Syntax

```lisp
(upsert-fm-field yaml-content key value)
```

## Description

`upsert-fm-field` modifies YAML frontmatter text by updating an existing field
or inserting a new one.  It operates on the raw YAML string, not a parsed
object.

## Arguments

- **yaml-content**: A string containing YAML frontmatter.
- **key**: The field name to set.
- **value**: The value to assign.

## Returns

A new YAML string with the field updated or added.

## Examples

```lisp
;; Update existing field
(upsert-fm-field "title: Old\nstatus: draft" "title" "New Title")
;; => "title: New Title\nstatus: draft"

;; Insert new field
(upsert-fm-field "title: Hello" "author" "Alice")
;; => "title: Hello\nauthor: Alice"

;; Update document frontmatter
(let ((content (get-frontmatter-content doc))
      (updated (upsert-fm-field content "modified" "2024-01-01")))
  (set-frontmatter doc "yaml" updated))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |

## See Also

- [`get-fm-field`](get-fm-field.md) - Read a field
- [`remove-fm-field`](remove-fm-field.md) - Delete a field
- [`set-frontmatter`](set-frontmatter.md) - Set entire frontmatter
