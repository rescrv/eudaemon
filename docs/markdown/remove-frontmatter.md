# remove-frontmatter

Remove frontmatter from a markdown document.

## Syntax

```lisp
(remove-frontmatter doc)
```

## Description

`remove-frontmatter` returns a new document with the frontmatter removed.  If
the document has no frontmatter, it is returned unchanged.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

A new document without frontmatter.

## Examples

```lisp
;; Remove existing frontmatter
(remove-frontmatter
  (markdown-to-sexpr "---\ntitle: Hello\n---\n\n# Content"))
;; => (doc (h1 "Content"))

;; No-op if no frontmatter
(remove-frontmatter (markdown-to-sexpr "# Just Content"))
;; => (doc (h1 "Just Content"))

;; In a pipeline
(->> doc
     (remove-frontmatter)
     (set-frontmatter "yaml" "clean: true"))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`get-frontmatter`](get-frontmatter.md) - Check if frontmatter exists
- [`set-frontmatter`](set-frontmatter.md) - Replace with new frontmatter
