# markdown-to-sexpr

Parse markdown text into an s-expression AST.

## Syntax

```lisp
(markdown-to-sexpr markdown-string)
```

## Description

`markdown-to-sexpr` parses a markdown string and returns its abstract syntax
tree as an s-expression.  The result is a `(doc ...)` form containing child
elements representing the markdown structure: headings, paragraphs, lists,
code blocks, and other elements.

This is the entry point for programmatic markdown manipulation—parse first,
transform, then render back with `sexpr-to-markdown`.

## Arguments

- **markdown-string**: A string containing markdown text.

## Returns

An s-expression AST with `doc` as the root tag.

## Examples

```lisp
;; Parse a simple heading
(markdown-to-sexpr "# Hello World")
;; => (doc (h1 "Hello World"))

;; Parse multiple elements
(markdown-to-sexpr "# Title\n\nSome text.\n\n- Item 1\n- Item 2")
;; => (doc (h1 "Title") (p "Some text.") (ul (li "Item 1") (li "Item 2")))

;; Parse with frontmatter
(markdown-to-sexpr "---\ntitle: My Doc\n---\n\n# Content")
;; => (doc (frontmatter "yaml" "title: My Doc") (h1 "Content"))

;; Use in pipeline
(->> "# Title\n\nText here"
     (markdown-to-sexpr)
     (generate-toc))
```

## AST Node Types

| Tag | Markdown | Example |
|-----|----------|---------|
| `doc` | Document root | `(doc ...)` |
| `h1`-`h6` | Headings | `(h1 "Title")` |
| `p` | Paragraph | `(p "Text")` |
| `ul` | Unordered list | `(ul (li ...) ...)` |
| `ol` | Ordered list | `(ol (li ...) ...)` |
| `li` | List item | `(li "Item")` |
| `code-block` | Fenced code | `(code-block "rust" "...")` |
| `code` | Inline code | `(code "expr")` |
| `blockquote` | Block quote | `(blockquote (p "..."))` |
| `link` | Link | `(link "url" "title" "text")` |
| `img` | Image | `(img "src" "alt" "title")` |
| `strong` | Bold | `(strong "text")` |
| `em` | Italic | `(em "text")` |
| `hr` | Horizontal rule | `(hr)` |
| `frontmatter` | YAML/TOML front | `(frontmatter "yaml" "...")` |

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`sexpr-to-markdown`](sexpr-to-markdown.md) - Convert AST back to markdown
- [`annotate`](annotate.md) - Add path IDs to nodes
