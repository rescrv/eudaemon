# scan-link-defs

Find all link definitions (reference-style link targets) in a document.

## Syntax

```lisp
(scan-link-defs doc)
```

## Description

`scan-link-defs` finds all reference-style link definitions in a document.
These are the `[identifier]: url "title"` lines that define targets for
reference links like `[text][identifier]`.

## Arguments

- **doc**: A markdown document s-expression.

## Returns

A list of link definition objects, each containing:
- Identifier (the reference name)
- URL
- Title (may be empty)
- Path in the document

## Examples

```lisp
;; Find link definitions
(scan-link-defs
  (markdown-to-sexpr "[site]: https://example.com \"Example\"\n\n[Go to site][site]"))
;; => ((link-def "site" "https://example.com" "Example" "1"))

;; List all defined identifiers
(map link-def-id (scan-link-defs doc))

;; Find unused definitions
(let ((defs (scan-link-defs doc))
      (refs (find-all-refs doc)))
  (filter (lambda (def) (not (member (link-def-id def) refs))) defs))
```

## Link Definition Structure

```lisp
(link-def identifier url title path)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |

## See Also

- [`find-undef-refs`](find-undef-refs.md) - Find references without definitions
- [`scan-links`](scan-links.md) - Find link usages
