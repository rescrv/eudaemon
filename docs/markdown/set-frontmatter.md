# set-frontmatter

Set or replace the frontmatter in a markdown document.

## Syntax

```lisp
(set-frontmatter doc format content)
```

## Description

`set-frontmatter` returns a new document with the specified frontmatter.  If
the document already has frontmatter, it is replaced; otherwise, frontmatter
is added at the beginning.

## Arguments

- **doc**: A markdown document s-expression.
- **format**: The frontmatter format, either `"yaml"` or `"toml"`.
- **content**: The frontmatter text content (without delimiters).

## Returns

A new document with the frontmatter set.

## Examples

```lisp
;; Add frontmatter to document without it
(set-frontmatter
  (markdown-to-sexpr "# Hello")
  "yaml"
  "title: Hello World")
;; => (doc (frontmatter "yaml" "title: Hello World") (h1 "Hello"))

;; Replace existing frontmatter
(set-frontmatter
  (markdown-to-sexpr "---\nold: data\n---\n\n# Content")
  "yaml"
  "new: data")
;; => (doc (frontmatter "yaml" "new: data") (h1 "Content"))

;; Set TOML frontmatter
(set-frontmatter doc "toml" "title = \"My Doc\"")
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |

## See Also

- [`get-frontmatter`](get-frontmatter.md) - Retrieve frontmatter
- [`remove-frontmatter`](remove-frontmatter.md) - Remove frontmatter
- [`upsert-fm-field`](upsert-fm-field.md) - Modify individual fields
