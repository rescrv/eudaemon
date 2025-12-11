# list-files

List all markdown files in the filesystem.

## Syntax

```lisp
(list-files)
```

## Description

`list-files` returns a list of all markdown files (`.md` extension) in the
VM's filesystem root, including files in subdirectories. Paths are returned
relative to the root.

This is useful for batch processing or discovering available documents.

## Arguments

None.

## Returns

A list of string paths to markdown files.

## Examples

```lisp
;; List all markdown files
(list-files)
;; => ("readme.md" "docs/guide.md" "notes/todo.md")

;; Process all files
(map load (list-files))

;; Filter by pattern (using string operations)
(filter
  (lambda (f) (contains? f "docs/"))
  (list-files))

;; Count markdown files
(length (list-files))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Any arguments provided |
| `no-filesystem` | No filesystem attached to VM |
| `io-error` | Error reading directory |

## See Also

- [`load`](load.md) - Load a specific file
- [`file-exists?`](file-exists-p.md) - Check if file exists
