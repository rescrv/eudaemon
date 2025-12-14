# slugify

Convert text into a URL-friendly slug.

## Syntax

```lisp
(slugify text)
```

## Description

`slugify` transforms a string into a URL-safe slug suitable for use in
filenames, anchor links, and URL paths.  The transformation:
- Converts to lowercase
- Replaces spaces and hyphens with hyphens
- Replaces other non-alphanumeric characters with underscores
- Collapses multiple hyphens into single hyphens
- Removes leading and trailing hyphens

## Arguments

- **text**: A string to slugify.

## Returns

A URL-friendly string.

## Examples

```lisp
;; Basic slugification
(slugify "Hello World")
;; => "hello-world"

;; Handles special characters
(slugify "API v2.0")
;; => "api-v2_0"

;; Collapses multiple spaces
(slugify "  Multiple   Spaces  ")
;; => "multiple-spaces"

;; Use with section titles for filenames
(let ((sections (extract-sections doc)))
  (map (lambda (s) (get s "slug")) sections))

;; Generate anchor link
(let ((title "Getting Started"))
  (concat "#" (slugify title)))
;; => "#getting-started"
```

## Notes

- The slug field in sections from `extract-sections` uses this same algorithm
- Suitable for generating filesystem-safe filenames
- Compatible with common markdown anchor link formats

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |
| `type-error` | Argument is not a string |

## See Also

- [`extract-sections`](extract-sections.md) - Returns sections with pre-computed slugs
- [`generate-toc`](generate-toc.md) - Uses slugs for anchor links
