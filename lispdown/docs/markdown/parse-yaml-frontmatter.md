# parse-yaml-frontmatter

Parse YAML frontmatter text into a structured object.

## Syntax

```lisp
(parse-yaml-frontmatter yaml-string)
```

## Description

`parse-yaml-frontmatter` parses a YAML string (typically from frontmatter)
into a JSON-like object s-expression.  This enables programmatic access to
individual frontmatter fields.

## Arguments

- **yaml-string**: A string containing YAML content.

## Returns

An `(obj ...)` s-expression representing the parsed YAML.

## Examples

```lisp
;; Parse simple YAML
(parse-yaml-frontmatter "title: Hello\nauthor: Alice")
;; => (obj ("title" "Hello") ("author" "Alice"))

;; Parse from document
(let ((content (get-frontmatter-content doc)))
  (parse-yaml-frontmatter content))

;; Access parsed field
(let ((fm (parse-yaml-frontmatter "title: My Doc\nstatus: draft")))
  (get fm "status"))
;; => "draft"

;; Pipeline for field access
(->> doc
     (get-frontmatter-content)
     (parse-yaml-frontmatter)
     (get "title"))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## Notes

Only simple key-value YAML is fully supported.  Nested structures may have
limited support.

## See Also

- [`get-frontmatter-content`](get-frontmatter-content.md) - Get YAML text
- [`get-fm-field`](get-fm-field.md) - Get field from parsed frontmatter
