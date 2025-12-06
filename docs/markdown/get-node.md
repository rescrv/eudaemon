# get-node

Retrieve a node by path identifier or content hash.

## Syntax

```lisp
(get-node doc node-id)
```

## Description

`get-node` locates a node in the document using either a path identifier (like
`"1.2"`) or a content-based identifier.  This provides flexible node lookup
that can survive minor document restructuring when using content IDs.

## Arguments

- **doc**: A markdown document s-expression.
- **node-id**: A path string or content identifier.

## Returns

- The matched node if found
- `null` if no node matches the identifier

## Examples

```lisp
;; Get by path
(get-node doc "1")
;; => (h1 "Title")

;; Get by path (nested)
(get-node doc "2.1")
;; => (li "First item")

;; Returns node without path info
(get-node doc "1.2.3")
;; => (actual node content)
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |

## See Also

- [`get-by-path`](get-by-path.md) - Path-only lookup
- [`get-parent`](get-parent.md) - Navigate to parent
- [`get-context`](get-context.md) - Get surrounding nodes
