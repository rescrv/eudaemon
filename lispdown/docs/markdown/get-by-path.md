# get-by-path

Retrieve a node from a document by its path identifier.

## Syntax

```lisp
(get-by-path doc path)
```

## Description

`get-by-path` navigates the document AST using a dot-separated path of indices
to locate and return a specific node.  Paths identify nodes by their position
in the tree structure, where each index represents a child position (1-indexed
for children, since index 0 is the tag).

## Arguments

- **doc**: A markdown document s-expression.
- **path**: A dot-separated path string (e.g., `"1"`, `"1.2"`, `"2.1.3"`).

## Returns

- The node at the specified path
- `null` if the path is invalid or doesn't exist

## Examples

```lisp
;; Get first child of document
(get-by-path (markdown-to-sexpr "# Hello\n\nWorld") "1")
;; => (h1 "Hello")

;; Get second child
(get-by-path (markdown-to-sexpr "# Hello\n\nWorld") "2")
;; => (p "World")

;; Navigate nested structure
(get-by-path
  (markdown-to-sexpr "- Item 1\n- Item 2")
  "1.1")
;; => (li "Item 1")

;; Invalid path returns null
(get-by-path doc "99")
;; => null

;; Use with annotate to find paths
(->> doc (annotate))  ;; Shows paths like (@1 ...), (@1.2 ...)
```

## Path Format

Paths are dot-separated sequences of indices:
- `"1"` - First child of root
- `"2"` - Second child of root
- `"1.1"` - First child of first child
- `"1.2.3"` - Third child of second child of first child

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |

## See Also

- [`get-node`](get-node.md) - Get by path or content ID
- [`get-parent`](get-parent.md) - Get parent of a node
- [`get-siblings`](get-siblings.md) - Get sibling nodes
- [`annotate`](annotate.md) - See all paths in document
- [`replace-at`](replace-at.md) - Modify node at path
