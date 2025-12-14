# wrap-in-callout

Wrap nodes in a blockquote callout with a type and optional title.

## Syntax

```lisp
(wrap-in-callout doc paths callout-type title)
```

## Description

`wrap-in-callout` wraps one or more nodes in a blockquote-based callout.  The
callout includes a type indicator (like "warning", "info", "note") and an
optional title.  This is useful for highlighting important information or
deprecation notices.

## Arguments

- **doc**: A markdown document s-expression.
- **paths**: A list of path strings, or a single path string.
- **callout-type**: A string like `"warning"`, `"info"`, `"note"`, `"tip"`.
- **title**: A title string, or empty string for no title.

## Returns

A new document with the nodes wrapped in a callout.

## Examples

```lisp
;; Wrap paragraph in warning
(wrap-in-callout
  (markdown-to-sexpr "# Title\n\nDangerous content here.")
  "2"
  "warning"
  "Caution")
;; Produces blockquote with [!warning] Caution header

;; Wrap multiple nodes
(wrap-in-callout doc (quote ("2" "3" "4")) "info" "Details")

;; No title
(wrap-in-callout doc "3" "note" "")

;; Deprecation notice
(wrap-in-callout doc "2" "warning" "Deprecated API")
```

## Rendered Output

The callout renders as a blockquote with GitHub-style admonition syntax:

```markdown
> [!warning] Caution
> Dangerous content here.
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly four arguments |
| `path-not-found` | One or more paths don't exist |

## See Also

- [`wrap-in-details`](wrap-in-details.md) - Collapsible wrapper
- [`mark-deprecated`](mark-deprecated.md) - Structured deprecation
