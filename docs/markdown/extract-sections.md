# extract-sections

Parse a document into a tree of sections based on header hierarchy.

## Syntax

```lisp
(extract-sections doc)
```

## Description

`extract-sections` analyzes a markdown document and splits it into a
hierarchical tree of sections based on header structure.  Each header becomes
a section boundary, with content between headers belonging to the preceding
header's section.  Headers with higher levels (e.g., h2, h3) become children
of the nearest preceding header with a lower level.

This function is the foundation for document refactoring operations like
splitting a monolithic document into multiple files organized by topic.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

An array of section objects.  Each section has:
- `level`: the header level (1-6)
- `title`: the header text
- `slug`: URL-friendly version of the title
- `content`: array of content nodes (not including child sections)
- `children`: array of child sections

Content appearing before the first header is ignored.

## Examples

```lisp
;; Parse a simple document
(extract-sections
  (markdown-to-sexpr "# Title\n\nIntro text.\n\n## Section 1\n\nContent 1.\n\n## Section 2\n\nContent 2."))
;; => (arr
;;      (section
;;        ("level" 1)
;;        ("title" "Title")
;;        ("slug" "title")
;;        ("content" (arr (p "Intro text.")))
;;        ("children" (arr
;;          (section ("level" 2) ("title" "Section 1") ...)
;;          (section ("level" 2) ("title" "Section 2") ...)))))

;; Extract sections and convert first to standalone doc
(let ((sections (extract-sections doc)))
  (section-to-doc (nth sections 0)))

;; Use with map to process all sections
(->> doc
     (extract-sections)
     (map process-section))
```

## Notes

- The returned structure mirrors the logical document hierarchy
- Sections can be converted back to documents using `section-to-doc`
- The `slug` field is suitable for generating filenames
- Deeply nested headers create deeply nested section structures

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`section-to-doc`](section-to-doc.md) - Convert a section back to a document
- [`slugify`](slugify.md) - Generate URL-friendly slugs
- [`generate-toc`](generate-toc.md) - Generate table of contents
- [`normalize-headers`](normalize-headers.md) - Fix heading structure first
