We're building a standard library for processing markdown.  The goal is to represent markdown as an
AST and then implement the following standard library calls so that a knowledge-base curation agent
can do the right thing.

Here is a proposed instruction set (ISA) designed for high-level semantic curation rather than low-level string manipulation.

### 1\. Context & Navigation (The "pwd" of the AST)

The LLM needs to know "where" it is or how to target specific sections without hallucinatory regex.

  * `(select selector)`: Returns a list of node IDs matching a CSS-like selector (e.g., `h1`, `table`, `#introduction`).
  * `(get-context node-id radius)`: Returns the siblings surrounding a node to ensure semantic coherence before editing.
  * `(scan-links)`: Returns an adjacency list of all internal/external links (crucial for "wiki gardening").

### 2\. Atomic Mutation (The Primitives)

These should strictly enforce Markdown validity (e.g., preventing a header inside a list item if your AST forbids it).

  * `(hoist node-id level)`: Promotes/demotes headers (e.g., changing `###` to `##`).
  * `(graft parent-id child-node index)`: Moves a subtree from one location to another.
  * `(prune node-id)`: Removes a subtree (e.g., deprecating an old section).
  * `(upsert-frontmatter key value)`: Essential for corporate wikis (tags, owners, last-modified dates).

### 3\. High-Level Curation (The "Curation" Verbs)

This is where the value lies. Give the LLM tools that encapsulate complex logic.

  * `(wrap-in-callout node-ids type "Title")`: Takes a range of nodes and wraps them in a blockquote/callout (e.g., "Warning" or "Info" boxes).
  * `(normalize-headers depth-map)`: Enforces a standard outline structure across disparate documents.
  * `(extract-to-ref target-doc-id node-ids)`: The "refactoring" verb. Moves content to a new document and leaves a link/summary behind.
  * `(merge-sections source-id target-id strategy)`: Merges two sections, with a strategy enum (`append`, `prepend`, or `llm-synthesize`).

### Example Workflow

If the LLM detects a deprecated API section, it might emit:

```lisp
(begin
  (upsert-frontmatter "status" "deprecated")
  (wrap-in-callout (select "#api-v1-usage") "warning" "Deprecation Notice")
  (prepend-child (select "#api-v1-usage") 
                 (paragraph "This API is deprecated. See " (link "v2" "./v2.md")))
)
```

### Recommendation

Implement a **transactional** model. The LLM submits a list of s-expressions; your system parses them into an execution plan, validates that the `node-ids` still exist (handling race conditions if the wiki is live), and then commits the AST transformation.
