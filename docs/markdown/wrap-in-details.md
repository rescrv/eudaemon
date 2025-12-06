# wrap-in-details

Wrap nodes in an HTML details/summary element for collapsible content.

## Syntax

```lisp
(wrap-in-details doc paths summary)
```

## Description

`wrap-in-details` wraps one or more nodes in an HTML `<details>` element with
a `<summary>`.  This creates collapsible content that readers can expand or
collapse.  Useful for hiding verbose details or optional information.

## Arguments

- **doc**: A markdown document s-expression.
- **paths**: A list of path strings, or a single path string.
- **summary**: The text shown in the clickable summary.

## Returns

A new document with the nodes wrapped in a details element.

## Examples

```lisp
;; Wrap verbose section
(wrap-in-details
  (markdown-to-sexpr "# Setup\n\nLong instructions here...")
  "2"
  "Click to expand installation steps")

;; Wrap multiple nodes
(wrap-in-details doc (quote ("3" "4" "5")) "Additional examples")

;; Hide implementation details
(wrap-in-details doc "4" "Show source code")
```

## Rendered Output

```html
<details>
<summary>Click to expand installation steps</summary>

Long instructions here...

</details>
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly three arguments |
| `path-not-found` | One or more paths don't exist |

## See Also

- [`wrap-in-callout`](wrap-in-callout.md) - Highlighted callout boxes
