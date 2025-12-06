# update-link

Update the URL of a link at a specific path.

## Syntax

```lisp
(update-link doc path new-url)
```

## Description

`update-link` returns a new document with the URL of the link at the specified
path changed to the new URL.  The link text and title are preserved.  This is
useful for fixing broken links or updating URLs during migration.

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A path string identifying a link or image node.
- **new-url**: The new URL to set.

## Returns

A new document with the link URL updated.

## Examples

```lisp
;; Update a link URL
(update-link
  (markdown-to-sexpr "[Click](./old-page.md)")
  "1.1"
  "./new-page.md")
;; => (doc (p (link "./new-page.md" "" "Click")))

;; Fix broken link from scan
(let ((broken (first (find-undef-refs doc))))
  (update-link doc (link-path broken) "./fixed.md"))

;; Update image source
(update-link doc "3.1" "./new-image.png")

;; Batch update in pipeline
(reduce
  (lambda (d link)
    (update-link d (link-path link) (fix-url (link-url link))))
  doc
  (get-internal-links doc))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `path-not-found` | Path doesn't exist |
| `not-a-link` | Node at path is not a link or image |

## See Also

- [`scan-links`](scan-links.md) - Find links to update
- [`get-internal-links`](get-internal-links.md) - Find internal links
- [`replace-at`](replace-at.md) - General node replacement
