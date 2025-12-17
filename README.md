# Eudaemon

Eudaemon is an integrated environment for creating sandboxed, programmable execution contexts
designed for AI agents and automated systems.  The project provides a self-contained filesystem,
shell, and document manipulation system that can operate entirely in-memory or backed by persistent
storage, enabling AI agents to work freely within well-defined boundaries while maintaining
deterministic, reproducible behavior.

The name "eudaemon" derives from the Greek concept of a good spirit or guiding daemon.  In the
context of this project, it represents infrastructure for building trustworthy agents that can
manipulate documents and filesystems safely, without risk to the host system.

Where traditional sandboxes focus on restriction, eudaemon focuses on enablement: providing rich
capabilities within safe confines.

## The Vision

Modern AI agents need more than just text generation.
- They need to read, write, organize, and transform information across documents and file
  structures.
- They need to execute commands, run scripts, and automate workflows.
- They need to do all of this safely, reproducibly, and without requiring trust in the agent's every
  action.

Eudaemon addresses these needs by providing a complete operating environment that can be
instantiated, manipulated, snapshotted, and destroyed without touching the host system.

An agent operating within eudaemon has access to an ever-improving POSIX-like shell with dozens of
commands, a filesystem that behaves like any Unix system, and a Lisp-based document manipulation
language purpose-built for programmatic knowledge base curation.

The agent can work freely, make mistakes, experiment, and iterate, all within an environment that
can be reset to a known state at any moment.

This architecture enables new patterns for AI-assisted work.

Consider a documentation agent that can traverse a knowledge base, identify inconsistencies,
restructure content, and generate cross-references, all using familiar shell idioms and powerful
document selectors.

Or a code review agent that can check out repositories, run builds, and analyze results in
isolation.

Or a research agent that can organize findings into structured documents, maintaining provenance and
enabling reproducible analysis.

The key insight is that agents benefit from the same tools humans have developed over decades of
computing: filesystems for organization, shells for automation, and domain-specific languages for
specialized tasks.

Eudaemon brings these tools into a form that agents can use safely and that operators can trust.

## Architecture

Eudaemon comprises four primary components that build upon each other to provide a complete execution environment.

### eudaemonty: The Foundation

At the base lies eudaemonty, a library of core traits and types that define the abstractions used
throughout the system.  The central abstraction is the `Filesystem` trait, which specifies the
complete interface for file operations: reading, writing, creating directories, managing symbolic
links, and querying metadata.  Alongside this are traits for standard I/O streams (`Stdin`,
`Stdout`, `Stderr`) that allow commands to operate uniformly regardless of whether they're connected
to real terminals, strings for testing, or network connections.

These traits enable the entire system to be polymorphic over its execution context.  The same shell
command that operates on real files during development can operate on an in-memory filesystem during
testing or within a sandboxed directory in production.  This uniformity extends throughout the
stack, making eudaemon components composable and testable.

### eudaemonfs: The Log-Structured Filesystem

Building on eudaemonty's `Filesystem` trait, eudaemonfs provides a complete log-structured
filesystem implementation.  Log-structured filesystems write all modifications sequentially to a
log, which provides several advantages for eudaemon's use case: operations are naturally atomic, the
filesystem state at any point can be reconstructed from the log, and the implementation can run
entirely in memory without complex journaling.

The filesystem supports the full range of Unix file semantics: regular files, directories, and
symbolic links; file descriptors that survive unlinking; metadata including timestamps and link
counts; and files up to one quarter of the total filesystem size through a combination of direct,
indirect, and double-indirect block addressing.  Block devices can be backed by memory for ephemeral
operation or by files for persistence, and wrapper devices enable sequential write enforcement for
testing and operation logging for debugging.

For AI agent workloads, eudaemonfs provides isolation without virtualization overhead.  Each agent
can have its own filesystem instance, completely separate from the host and from other agents.  The
filesystem can eventually be initialized with specific content, allowed to evolve through agent
actions, and then inspected or discarded as needed.

### eudaemonsh: The Shell Environment

With a filesystem in place, eudaemonsh provides the command execution layer.  It implements over
forty-five builtin commands covering file operations (`cat`, `cp`, `mv`, `rm`, `mkdir`, `ln`), text
processing (`cut`, `grep`, `head`, `tail`, `sort`, `uniq`, `wc`, `tr`), directory navigation (`ls`,
`pwd`, `du`), and utilities (`echo`, `date`, `env`, `printf`, `base64`).  Each command is
implemented against the abstract `Filesystem` trait, meaning the same commands work transparently
across real filesystems, sandboxed directories, and in-memory eudaemonfs instances.

The shell environment bundles these commands with standard I/O streams, environment variables, and
working directory state into a coherent execution context.  Commands receive this context and
produce output and exit codes just as they would in a traditional Unix shell.  For testing and agent
integration, the I/O streams can be backed by strings, enabling deterministic verification of
command behavior.

The project used FreeBSD as a base and liberally borrowed from BSD licensed code.  Over fifteen
hundred test cases covering builtins, expansion, parsing, and execution ensure that eudaemonsh's
behavior matches established POSIX shell semantics.  This compatibility means agents can use
familiar shell patterns and scripts, and humans can reason about agent behavior using their existing
Unix knowledge.

### lispdown: The Document Manipulation Language

While the shell provides general-purpose automation, lispdown addresses the specific needs of
programmatic document manipulation.  It implements a Lisp dialect purpose-built for transforming
markdown documents, with bidirectional conversion between markdown and S-expression abstract syntax
trees.

A markdown document like:

```markdown
# Introduction

Welcome to the guide.

## Getting Started

First, install the dependencies.
```

becomes an S-expression:

```lisp
(doc
  (h1 "Introduction")
  (p "Welcome to the guide.")
  (h2 "Getting Started")
  (p "First, install the dependencies."))
```

In this form, documents become data structures that can be queried, transformed, and reassembled
using the full power of a Lisp environment.  Lispdown provides over eighty builtin functions for
document manipulation: navigation (`get-by-path`, `get-parent`, `get-siblings`), mutation
(`replace-at`, `graft`, `prune`, `insert-before`, `insert-after`), curation (`generate-toc`,
`wrap-in-callout`, `mark-deprecated`, `extract-sections`), and analysis (`scan-links`,
`find-undefined-references`).

The system supports two complementary addressing schemes for document elements.  Path-based
addressing uses dot-separated indices (like "1.2.3") for positional reference, suitable for
navigation and display.  Content-based addressing uses SHA3 hashes of element content, providing
stable references that survive document reorganization.  This dual addressing enables both
human-readable references and robust programmatic links.

Lispdown includes a condition system inspired by Common Lisp, allowing fine-grained error handling
with restarts that can recover from failures in user-specified ways.  The VM supports steppable
execution for debugging and hot-swappable function definitions for interactive development.  A REPL
with autocomplete enables exploratory programming, while batch mode supports scripted document
transformations.

## Usage Patterns

Eudaemon supports several usage patterns depending on the degree of isolation and persistence
required.

For maximum isolation, instantiate an in-memory eudaemonfs, populate it with initial content, and
provide it to an agent.  The agent operates entirely within this synthetic filesystem, with no
access to the host system whatsoever.  When the agent completes, extract any results and destroy the
filesystem, or snapshot it for later resumption.

For sandboxed access to real files, use `DirectoryFilesystem` from eudaemonty, which confines
operations to a specific directory and rejects any path traversal attempts.  The agent works with
real files but cannot escape its designated workspace.  This pattern suits scenarios where the agent
needs to interact with actual project files but should not access other system resources.

For document manipulation workflows, use lispdown either standalone or integrated with the shell.
Documents can be loaded, transformed through Lisp expressions, and written back, all with the safety
guarantees of the underlying filesystem abstraction.  The shell's `markdownsp` builtin provides a
bridge between shell pipelines and document manipulation.

For testing and development, all components support string-backed I/O and mock filesystems.  Tests
can verify exact command output, filesystem state changes, and document transformations without
touching any real resources.  The deterministic time sources throughout the stack ensure
reproducible behavior across test runs.

## Current State

Eudaemon is functional and actively developed.  The filesystem implementation handles the full range
of Unix file operations with property-based testing ensuring correctness.  The shell provides a
useful subset of standard Unix commands with extensive test coverage.  The document manipulation
language supports sophisticated transformations with a complete condition system.

Areas of ongoing development include expanding shell command coverage, optimizing filesystem
performance for large workloads, and deepening integration between components.  The architecture is
stable and the APIs are approaching their final form, though refinements continue as real-world
usage reveals opportunities for improvement.

The project uses Rust edition 2024 and embraces modern Rust idioms throughout.  Trait-based
abstraction enables composition and testing, while careful attention to error handling ensures that
failures are reported clearly and can be handled gracefully.

## Building and Testing

The project uses Cargo for building and testing:

```bash
# Build all components
cargo build --all

# Run tests
cargo test

# Run with additional lints
cargo clippy --all-targets -- -D clippy::all
```

Each component can also be built and tested individually by specifying its directory.

## Design Philosophy

Several principles guide eudaemon's design.

Abstraction through traits enables the same code to operate across different contexts without
modification.  A shell command written against the `Filesystem` trait works equally well on real
files, sandboxed directories, and in-memory filesystems.  This uniformity reduces complexity and
increases confidence in behavior across deployment scenarios.

Composition over inheritance allows components to be combined flexibly.  The shell doesn't inherit
from the filesystem; it accepts any filesystem implementation.  Lispdown doesn't inherit from the
shell; it can operate independently or integrate through well-defined interfaces.  This loose
coupling enables diverse usage patterns without architectural constraints.

Determinism enables reproducibility.  Time sources are injectable functions rather than system
calls.  Random number generation, when needed, uses seedable generators.  I/O can be captured and
replayed.  These properties make eudaemon suitable for testing, debugging, and scenarios where exact
reproducibility matters.

Safety through isolation protects both the host system and the agent.  The host is protected because
agent actions cannot escape the sandbox.  The agent is protected because its environment is
well-defined and predictable, free from interference by other processes or system state changes.
