# remove-fm-field

Remove a field from YAML frontmatter text.

## Syntax

```lisp
(remove-fm-field yaml-content key)
```

## Description

`remove-fm-field` removes a field from YAML frontmatter text.  If the field
does not exist, the content is returned unchanged.

## Arguments

- **yaml-content**: A string containing YAML frontmatter.
- **key**: The field name to remove.

## Returns

A new YAML string with the field removed.

## Examples

```lisp
;; Remove existing field
(remove-fm-field "title: Hello\nstatus: draft\nauthor: Alice" "status")
;; => "title: Hello\nauthor: Alice"

;; Remove non-existent field (unchanged)
(remove-fm-field "title: Hello" "missing")
;; => "title: Hello"

;; Update document to remove field
(let ((content (get-frontmatter-content doc))
      (updated (remove-fm-field content "deprecated")))
  (set-frontmatter doc "yaml" updated))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |

## See Also

- [`get-fm-field`](get-fm-field.md) - Check if field exists first
- [`upsert-fm-field`](upsert-fm-field.md) - Add or update a field
