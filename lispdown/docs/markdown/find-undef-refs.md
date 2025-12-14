# find-undef-refs

Find reference-style links that have no matching definition.

## Syntax

```lisp
(find-undef-refs doc)
```

## Description

`find-undef-refs` finds all reference-style links (like `[text][ref]` or
`[ref][]`) that reference identifiers without corresponding definitions.
These are broken links that will render as literal text.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

A list of link info objects for undefined references.

## Examples

```lisp
;; Find undefined references
(find-undef-refs
  (markdown-to-sexpr "[Click][missing]\n\n[defined]: https://example.com"))
;; => ((link-info "link-ref" "missing" "Click" "1.1"))

;; Check if document has broken refs
(if (empty? (find-undef-refs doc))
    "All references defined"
    "Has broken references")

;; List missing identifiers
(map link-ref (find-undef-refs doc))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`scan-link-defs`](scan-link-defs.md) - Find existing definitions
- [`scan-links`](scan-links.md) - Find all link usages
