# write-file

Write a string to a file.

## Syntax

```lisp
(write-file path content)
```

## Description

`write-file` writes a string directly to a file without any conversion. This
is useful for writing non-markdown files or raw content. Parent directories
are created automatically if they don't exist.

For saving s-expression documents as markdown, prefer `save` which handles
the conversion.

## Arguments

- **path**: A string path where the file should be written.
- **content**: The string content to write.

## Returns

The path string that was written to.

## Examples

```lisp
;; Write raw content
(write-file "output.txt" "Hello, world!")
;; => "output.txt"

;; Write JSON
(write-file "data.json" "{\"key\": \"value\"}")

;; Write to subdirectory
(write-file "output/result.txt" "processed data")

;; Copy file contents
(write-file "backup.md" (read-file "original.md"))

;; Generate and write
(write-file
  "toc.md"
  (sexpr-to-markdown (generate-toc (load "readme.md"))))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly two arguments |
| `no-filesystem` | No filesystem attached to VM |
| `io-error` | Error writing the file |
| `parent-dir-not-allowed` | Path contains `..` |

## See Also

- [`save`](save.md) - Save s-expression as markdown
- [`read-file`](read-file.md) - Read raw file content
