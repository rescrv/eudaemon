You asked me to think harder. The "CMS-style" CRUD operations (move, delete, rename) I listed previously are necessary but insufficient for *curation*. An LLM acting as a curator isn't just a janitor; it's an architect. It needs tools to manage **entropy** and **topology**, not just text.

### 1\. Topological Refactoring (The "Graph Doctor")

The hardest part of maintaining a wiki is that it is a *graph* of *trees*. Most tools treat it as a collection of files.

  * `(extract-subtree node-id target-path strategy)`:

      * **The Problem:** Monolithic documents ("God Classes" of documentation) rot because no one reads them.
      * **The Operation:** Atomically extracts a subtree (e.g., a specific H2 and its children) into a new markdown file.
      * **The `strategy` Enum:**
          * `'leave-link`: Replaces the original tree with `[See details](./new-path.md)`.
          * `'transclude`: Replaces it with a transclusion directive (if supported).
          * `'redirect`: Leaves a 301-style frontmatter redirect.

  * `(rehome-orphans source-id target-id)`:

      * **The Problem:** When you delete a section, incoming links break (dangling pointers).
      * **The Operation:** Finds all internal links pointing to `source-id` and updates them to point to `target-id`. This is essential for "merging" duplicate topics without breaking the knowledge graph.

### 2\. Stigmergic Annotation (The "Pheromones")

Give the LLM the ability to leave metadata for *future* agent passes (or human review) without polluting the visible text.

  * `(tag-node node-id key value visibility)`:

      * Attaches metadata to a specific AST node (not just file frontmatter).
      * *Implementation:* Can use HTML comments \`\` injected before/after the node, or a sidecar index.
      * *Use Case:* An LLM scans a doc, marks specific paragraphs as `ambiguous` or `needs-citation`. A second "writer" agent queries for that tag and fixes it.

  * `(assert-invariant selector predicate)`:

      * **The Problem:** Regression.
      * **The Operation:** "Ensure all H2s in 'API Docs' have a 'Returns' subsection." If the invariant fails, the interpreter rejects the commit or flags it. This allows the LLM to write unit tests for the documentation structure.

### 3\. The "Lens" (Token Economy Optimization)

You are likely paying per token. Don't make the LLM read the whole AST to find one paragraph.

  * `(focus-context selector context-window-size)`:

      * Returns a *pruned* AST containing only the matching nodes and their immediate ancestors/neighbors. This allows the LLM to perform "surgical" edits on a massive file without loading the entire context.

  * `(skeletonize depth)`:

      * Returns the document structure (headers, list density, link density) *without* the prose. The LLM can decide *where* to work based on the skeleton, then request the full text of just that section.

-----

### Is there a better language?

Lisp is correct for the **AST mutation** layer because it is homoiconic with the tree structure. However, for the **Query** layer (the `select` argument in my previous examples), Lisp is clumsy.

**The Hybrid Approach:**
Use **Lisp** for mutation, but embed a **Logic Query Language** for selection.

Look at **Datalog** (or a simplified subset).

Instead of:
`(select (and (type header) (depth 2) (contains "Legacy")))`

Use a Datalog-style query in the selector:

```lisp
(mutate 
  ;; The Selector (Datalog-ish)
  (query "find ?n where (header ?n) (depth ?n 2) (text-contains ?n 'Legacy')")
  
  ;; The Mutation (Lisp)
  (wrap-callout ?n "warning" "Legacy Content")
)
```

**Why Datalog?**

1.  **Pattern Matching:** It maps perfectly to the "Entity-Component-System" mental model you like (Nodes are entities, attributes are components).
2.  **Relational:** It handles the *graph* nature of the wiki (A links to B) better than CSS selectors or XPath.
3.  **Safety:** It terminates.

**Summary Recommendation:**
Stick to your Lisp interpreter, but implement a **unification engine** (Datalog-lite) for the `selector` logic. This gives your LLM the power to reason about relationships ("Find all headers that don't have a parent with tag X") before it performs surgery.
