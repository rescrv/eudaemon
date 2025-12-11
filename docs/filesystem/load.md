# load

Load a markdown file and parse it to an s-expression.

## Syntax

```lisp
(load path)
```

## Description

`load` reads a markdown file from the filesystem and parses it into an
s-expression document. The path is relative to the VM's filesystem root
(typically the REPL's working directory).

This is the primary way to load documents for programmatic manipulation.

## Arguments

- **path**: A string path to the markdown file, relative to the filesystem root.

## Returns

An s-expression representing the parsed markdown document.

## Examples

```lisp
;; Load a document
(load "readme.md")
;; => (doc (h1 "Title") (p "Content"))

;; Load and transform
(-> (load "notes.md")
    (prune "1.2")
    (save "notes.md"))

;; Load from subdirectory
(load "docs/guide.md")

;; Use with other functions
(get-frontmatter (load "post.md"))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |
| `no-filesystem` | No filesystem attached to VM |
| `not-found` | File does not exist |
| `io-error` | Error reading the file |
| `parent-dir-not-allowed` | Path contains `..` |

## See Also

- [`save`](save.md) - Write s-expression back to file
- [`read-file`](read-file.md) - Read raw file content
- [`list-files`](list-files.md) - List available files
