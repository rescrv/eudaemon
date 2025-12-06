# get-image-links

Get all image references from a document.

## Syntax

```lisp
(get-image-links doc)
```

## Description

`get-image-links` finds all images in a document, including inline images
(`![alt](url)`) and reference-style images (`![alt][ref]`).  This is useful
for asset management and validation.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

A list of link info objects for images only.

## Examples

```lisp
;; Get all images
(get-image-links
  (markdown-to-sexpr "![Logo](./logo.png)\n\n[Link](./page.md)"))
;; => ((link-info "img" "./logo.png" "Logo" "1.1"))

;; Find images without alt text
(filter (lambda (img) (empty? (img-alt img)))
        (get-image-links doc))

;; List all image URLs
(map link-url (get-image-links doc))

;; Check for external images
(filter (lambda (img) (starts-with (link-url img) "http"))
        (get-image-links doc))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`scan-links`](scan-links.md) - Get all links including images
- [`get-internal-links`](get-internal-links.md) - Internal links only
