# file-exists?

Check if a file exists.

## Syntax

```lisp
(file-exists? path)
```

## Description

`file-exists?` checks whether a file exists at the given path. Only regular
files return true; directories return false.

This is useful for conditional logic before loading or writing files.

## Arguments

- **path**: A string path to check, relative to the filesystem root.

## Returns

- `#t` if the file exists
- `#f` if the file does not exist or is a directory

## Examples

```lisp
;; Check existence
(file-exists? "readme.md")
;; => #t

;; Conditional load
(if (file-exists? "config.md")
    (load "config.md")
    (markdown-to-sexpr "# Default Config"))

;; Guard before operations
(if (file-exists? "draft.md")
    (save (prune (load "draft.md") "1") "draft.md")
    "File not found")

;; Check multiple files
(filter file-exists? (quote ("a.md" "b.md" "c.md")))
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |
| `no-filesystem` | No filesystem attached to VM |

## Notes

- Paths containing `..` return `#f` rather than an error
- Does not follow symlinks for the security check, only validates the path

## See Also

- [`load`](load.md) - Load file (errors if not found)
- [`list-files`](list-files.md) - List all available files
