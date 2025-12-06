# get-frontmatter-content

Extract the raw text content of frontmatter from a markdown document.

## Syntax

```lisp
(get-frontmatter-content doc)
```

## Description

`get-frontmatter-content` returns the text content of the frontmatter as a
string, without the format tag or delimiters.  This is useful for parsing or
manipulating the frontmatter content directly.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

- A string containing the frontmatter text if present
- `null` if the document has no frontmatter

## Examples

```lisp
;; Get frontmatter text
(get-frontmatter-content
  (markdown-to-sexpr "---\ntitle: Hello\nauthor: Alice\n---\n\n# Content"))
;; => "title: Hello\nauthor: Alice"

;; No frontmatter returns null
(get-frontmatter-content (markdown-to-sexpr "# No Front"))
;; => null

;; Parse the content as YAML
(parse-yaml-frontmatter
  (get-frontmatter-content doc))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`get-frontmatter`](get-frontmatter.md) - Get the full node with format
- [`parse-yaml-frontmatter`](parse-yaml-frontmatter.md) - Parse YAML content
