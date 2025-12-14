# scan-links

Find all links in a document and return them with metadata.

## Syntax

```lisp
(scan-links doc)
```

## Description

`scan-links` traverses a document and collects information about every link,
including regular links, images, and reference-style links.  Each link is
returned with its URL, text, type, and location path.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

A list of link info objects, each containing:
- Link type (`link`, `img`, `link-ref`, `img-ref`)
- URL or reference identifier
- Link text or alt text
- Path in the document

## Examples

```lisp
;; Scan document for links
(scan-links
  (markdown-to-sexpr "[Click here](./page.md)\n\n![Image](./pic.png)"))
;; => ((link-info "link" "./page.md" "Click here" "1.1")
;;     (link-info "img" "./pic.png" "Image" "2.1"))

;; Count total links
(length (scan-links doc))

;; Filter in pipeline
(->> doc
     (scan-links)
     (filter is-internal?))
```

## Link Info Structure

Each result is a tagged s-expression:
```lisp
(link-info type url text path)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`get-internal-links`](get-internal-links.md) - Filter to internal only
- [`get-external-links`](get-external-links.md) - Filter to external only
- [`get-image-links`](get-image-links.md) - Filter to images only
