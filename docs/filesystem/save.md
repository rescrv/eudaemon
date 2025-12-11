# save

Save an s-expression document to a markdown file.

## Syntax

```lisp
(save doc path)
```

## Description

`save` converts an s-expression document to markdown and writes it to the
specified file. The path is relative to the VM's filesystem root. Parent
directories are created automatically if they don't exist.

This is the complement to `load` for persisting document transformations.

## Arguments

- **doc**: An s-expression document to save.
- **path**: A string path where the file should be written.

## Returns

The path string that was written to.

## Examples

```lisp
;; Save a document
(save (load "draft.md") "final.md")
;; => "final.md"

;; Transform and save in place
(save (prune (load "readme.md") "1.2") "readme.md")

;; Create new document
(save
  (markdown-to-sexpr "# New Document\n\nContent here.")
  "new-file.md")

;; Save to subdirectory (creates it if needed)
(save doc "output/processed.md")
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `no-filesystem` | No filesystem attached to VM |
| `io-error` | Error writing the file |
| `parent-dir-not-allowed` | Path contains `..` |

## See Also

- [`load`](load.md) - Read markdown file to s-expression
- [`write-file`](write-file.md) - Write raw string content
- [`sexpr-to-markdown`](../markdown/sexpr-to-markdown.md) - Convert without saving
