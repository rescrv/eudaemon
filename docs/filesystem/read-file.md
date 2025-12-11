# read-file

Read a file as a raw string.

## Syntax

```lisp
(read-file path)
```

## Description

`read-file` reads the contents of a file and returns it as a string without
parsing. This is useful for reading non-markdown files or when you need the
raw content before custom processing.

For markdown files that you want to manipulate programmatically, prefer `load`
which parses the content to an s-expression.

## Arguments

- **path**: A string path to the file, relative to the filesystem root.

## Returns

The file contents as a string.

## Examples

```lisp
;; Read raw content
(read-file "config.yaml")
;; => "key: value\nother: data"

;; Read and then parse
(markdown-to-sexpr (read-file "doc.md"))

;; Read for inspection
(print (read-file "template.txt"))

;; Compare to load (which parses)
(read-file "readme.md")   ;; => "# Title\n\nContent"
(load "readme.md")        ;; => (doc (h1 "Title") (p "Content"))
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

- [`load`](load.md) - Read and parse markdown
- [`write-file`](write-file.md) - Write raw string to file
- [`file-exists?`](file-exists-p.md) - Check before reading
