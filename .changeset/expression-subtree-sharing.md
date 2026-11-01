---
"@lumiscia/lumen-bindings": minor
---

Evaluate repeated expression subtrees once per evaluation, reducing CPU work during timeline-bounds preparation and frame binding. Values are shared only within the same evaluation; short-circuit branches, error ordering and contextual inputs retain their behavior. In Rust, read the expression tree with `Expression::ast()` instead of its former public `ast` field. Construct a custom tree with `Expression::new(ast, source)`, which derives references and its evaluation plan together. TypeScript and JSON expression usage are unchanged.
