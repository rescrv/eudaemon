# sexpr-to-markdown

Render an s-expression AST back to markdown text.

## Syntax

```lisp
(sexpr-to-markdown doc-expr)
```

## Description

`sexpr-to-markdown` converts an s-expression AST (as produced by
`markdown-to-sexpr`) back into markdown text.  This completes the
parse-transform-render cycle, allowing programmatic manipulation of markdown
documents.

The function preserves semantic structure but may not reproduce the exact
original formatting (e.g., whitespace, heading style ATX vs Setext).

## Arguments

- **doc-expr**: An s-expression AST, typically a `(doc ...)` form.

## Returns

A string containing the rendered markdown.

## Examples

```lisp
;; Render a simple document
(sexpr-to-markdown (doc (h1 "Title") (p "Hello world.")))
;; => "# Title\n\nHello world.\n"

;; Round-trip preservation
(let ((md "# Title\n\nParagraph text."))
  (sexpr-to-markdown (markdown-to-sexpr md)))
;; => "# Title\n\nParagraph text.\n"

;; Render after transformation
(->> "# Old Title"
     (markdown-to-sexpr)
     (replace-at "1" (h1 "New Title"))
     (sexpr-to-markdown))
;; => "# New Title\n"

;; Construct and render
(sexpr-to-markdown
  (quote (doc
    (h1 "Generated Doc")
    (p "This was built programmatically.")
    (ul (li "Item 1") (li "Item 2")))))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## Notes

The renderer produces clean, consistent markdown output.  Some formatting
choices (like ATX-style headings with `#`) are fixed regardless of the
original input format.

Whitespace and blank lines are normalized.  If you need exact round-trip
preservation of formatting, consider storing the original text.

## See Also

- [`markdown-to-sexpr`](markdown-to-sexpr.md) - Parse markdown to AST
- [`annotate`](annotate.md) - Inspect structure before rendering
