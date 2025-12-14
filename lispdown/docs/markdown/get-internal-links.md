# get-internal-links

Get all internal (relative) links from a document.

## Syntax

```lisp
(get-internal-links doc)
```

## Description

`get-internal-links` finds all links that point to internal destinations—
relative paths, anchor links, and local file references.  External links
(those starting with `http://`, `https://`, or other protocols) are excluded.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

A list of link info objects for internal links only.

## Examples

```lisp
;; Get internal links
(get-internal-links
  (markdown-to-sexpr "[Local](./other.md)\n\n[External](https://example.com)"))
;; => ((link-info "link" "./other.md" "Local" "1.1"))

;; Find broken internal links (compare with file list)
(let ((links (get-internal-links doc))
      (files (list-files)))
  (filter (lambda (link) (not (member (link-url link) files))) links))

;; Count internal links
(length (get-internal-links doc))
```

## Internal Link Detection

A link is considered internal if its URL:
- Starts with `./` or `../` (relative path)
- Starts with `#` (anchor link)
- Contains no protocol (e.g., `page.md`)
- Does not start with `http://`, `https://`, `mailto:`, etc.

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`get-external-links`](get-external-links.md) - Get external links
- [`scan-links`](scan-links.md) - Get all links
- [`update-link`](update-link.md) - Fix broken links
