# help

Display documentation for a command.

## Syntax

```lisp
(help command-name)
```

## Description

`help` retrieves the documentation for a named command.  The command name should
be provided as an unquoted atom.  Documentation is available for all builtin
functions, special forms, JSON operations, and markdown operations.

Special command names are handled appropriately:
- Predicates ending in `?` (e.g., `null?`, `empty?`, `list?`)
- Threading macros `->` and `->>`

## Arguments

- **command-name**: An atom naming the command to look up.

## Returns

A string containing the markdown documentation for the command.

## Examples

```lisp
;; Get help for a list operation
(help first)
;; => "# first\n\nExtract the first element of a list.\n..."

;; Get help for a predicate
(help null?)
;; => "# null?\n\nTest whether a value is null.\n..."

;; Get help for thread-first
(help ->)
;; => "# -> (thread-first)\n\n..."

;; Get help for a JSON operation
(help get)
;; => "# get\n\nRetrieve a value from a JSON object by key.\n..."
```

## Errors

| Code | Condition |
|------|-----------|
| `wrong-argument-count` | Not exactly one argument |
| `type-error` | Argument is not an atom |
| `command-not-found` | No documentation exists for the command |

## See Also

- All documented commands are available in the `docs/` directory
