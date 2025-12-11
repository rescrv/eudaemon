# section-to-doc

Convert a section object back into a standalone document.

## Syntax

```lisp
(section-to-doc section)
```

## Description

`section-to-doc` takes a section object (as returned by `extract-sections`) and
converts it into a standalone markdown document.  The section's title becomes
an h1 header, and all direct content is included.

Child sections are NOT included in the output.  This makes the function suitable
for extracting leaf sections or for processing a section hierarchy where each
level is written to a separate file.

## Arguments

- **section**: A section object from `extract-sections`.

## Returns

A document s-expression containing:
- The section title as an h1 header
- All content nodes from the section

## Examples

```lisp
;; Extract sections and convert first to a document
(let ((sections (extract-sections doc)))
  (section-to-doc (first (rest sections))))
;; => (doc (h1 "Section Title") (p "Content..."))

;; Write each section to its own file
(let ((sections (extract-sections doc)))
  (map (lambda (s)
         (let ((content (section-to-doc s))
               (filename (get s "slug")))
           (write-file filename (sexpr-to-markdown content))))
       sections))

;; Pipeline to extract and render a section
(->> doc
     (extract-sections)
     (nth 0)
     (section-to-doc)
     (sexpr-to-markdown))
```

## Notes

- The section's original header level is normalized to h1 in the output
- Child sections must be processed separately if needed
- Use `get` to access the section's `slug` field for filenames

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |
| `field-not-found` | Section is missing required fields |

## See Also

- [`extract-sections`](extract-sections.md) - Parse document into sections
- [`slugify`](slugify.md) - Generate URL-friendly slugs
- [`sexpr-to-markdown`](sexpr-to-markdown.md) - Convert back to markdown text
