# get-frontmatter

Extract the frontmatter node from a markdown document.

## Syntax

```lisp
(get-frontmatter doc)
```

## Description

`get-frontmatter` returns the frontmatter node from a markdown document AST,
if present.  Frontmatter is metadata at the beginning of a document, delimited
by `---` (YAML) or `+++` (TOML).

## Arguments

- **doc**: A markdown document s-expression.

## Returns

- The frontmatter node `(frontmatter "format" "content")` if present
- `null` if the document has no frontmatter

## Examples

```lisp
;; Get YAML frontmatter
(get-frontmatter
  (markdown-to-sexpr "---\ntitle: Hello\n---\n\n# Content"))
;; => (frontmatter "yaml" "title: Hello")

;; No frontmatter returns null
(get-frontmatter (markdown-to-sexpr "# Just Content"))
;; => null

;; Check for presence
(if (null? (get-frontmatter doc))
    "No frontmatter"
    "Has frontmatter")
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`get-frontmatter-content`](get-frontmatter-content.md) - Get just the text
- [`set-frontmatter`](set-frontmatter.md) - Add or replace frontmatter
- [`remove-frontmatter`](remove-frontmatter.md) - Remove frontmatter
