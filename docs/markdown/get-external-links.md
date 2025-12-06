# get-external-links

Get all external links from a document.

## Syntax

```lisp
(get-external-links doc)
```

## Description

`get-external-links` finds all links that point to external destinations—
URLs with protocols like `http://`, `https://`, `mailto:`, etc.  Internal
relative links are excluded.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

A list of link info objects for external links only.

## Examples

```lisp
;; Get external links
(get-external-links
  (markdown-to-sexpr "[Local](./other.md)\n\n[Web](https://example.com)"))
;; => ((link-info "link" "https://example.com" "Web" "2.1"))

;; List all external domains
(->> doc
     (get-external-links)
     (map link-url)
     (map extract-domain))

;; Check for http (non-https) links
(filter (lambda (link) (starts-with (link-url link) "http://"))
        (get-external-links doc))
```

## External Link Detection

A link is considered external if its URL:
- Starts with `http://` or `https://`
- Starts with other protocols like `mailto:`, `ftp://`, etc.

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`get-internal-links`](get-internal-links.md) - Get internal links
- [`scan-links`](scan-links.md) - Get all links
