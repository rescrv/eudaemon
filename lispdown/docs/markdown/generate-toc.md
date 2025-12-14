# generate-toc

Generate a table of contents from document headings.

## Syntax

```lisp
(generate-toc doc)
```

## Description

`generate-toc` analyzes a document's heading structure and generates a nested
list representing the table of contents.  Each heading becomes a list item
with a link to that section.  The nesting reflects the heading hierarchy.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

An s-expression representing a nested unordered list (table of contents).

## Examples

```lisp
;; Generate TOC for simple document
(generate-toc
  (markdown-to-sexpr "# Title\n\n## Section 1\n\n## Section 2\n\n### Subsection"))
;; => (ul
;;      (li (link "#title" "" "Title")
;;        (ul
;;          (li (link "#section-1" "" "Section 1"))
;;          (li (link "#section-2" "" "Section 2")
;;            (ul
;;              (li (link "#subsection" "" "Subsection")))))))

;; Insert TOC into document
(->> doc
     (generate-toc)
     (insert-after doc "1"))  ;; After title

;; Pipeline to render TOC as markdown
(->> "# Guide\n\n## Setup\n\n## Usage\n\n## FAQ"
     (markdown-to-sexpr)
     (generate-toc)
     (sexpr-to-markdown))
```

## Notes

- Heading anchors are generated from heading text (slugified)
- The TOC structure mirrors the logical heading hierarchy
- Skipped levels (e.g., h1 directly to h3) are handled gracefully

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`normalize-headers`](normalize-headers.md) - Fix heading structure first
- [`insert-after`](insert-after.md) - Insert TOC into document
- [`prepend-child`](prepend-child.md) - Add TOC at start of section
- [`sexpr-to-markdown`](sexpr-to-markdown.md) - Render TOC as markdown
