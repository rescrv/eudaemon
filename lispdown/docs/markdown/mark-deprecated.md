# mark-deprecated

Add a deprecation notice to nodes with optional reason and replacement link.

## Syntax

```lisp
(mark-deprecated doc paths reason replacement-link)
```

## Description

`mark-deprecated` adds a structured deprecation notice to one or more nodes.
The notice includes an optional reason explaining why the content is deprecated
and an optional link to the replacement.  This is a semantic operation for
maintaining wikis and documentation.

## Arguments

- **doc**: A markdown document s-expression.
- **paths**: A list of path strings, or a single path string.
- **reason**: A string explaining the deprecation, or empty string.
- **replacement-link**: A URL to the replacement, or empty string.

## Returns

A new document with deprecation notices added.

## Examples

```lisp
;; Mark section as deprecated with full info
(mark-deprecated
  (markdown-to-sexpr "# Old API\n\nDo things the old way.")
  "1"
  "This API is obsolete since v2.0"
  "./new-api.md")

;; Deprecate without replacement
(mark-deprecated doc "3" "No longer maintained" "")

;; Deprecate without reason
(mark-deprecated doc "2" "" "./alternative.md")

;; Mark multiple sections
(mark-deprecated doc
  (quote ("2" "4" "6"))
  "Legacy features"
  "./migration-guide.md")
```

## Output Structure

The deprecation notice is added as a warning callout before the deprecated
content, containing the reason and link if provided.

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly four arguments |
| `path-not-found` | One or more paths don't exist |

## See Also

- [`wrap-in-callout`](wrap-in-callout.md) - General callout wrapping
- [`upsert-fm-field`](upsert-fm-field.md) - Mark status in frontmatter
