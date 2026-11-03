---
"@lumiscia/lumen-bindings": minor
---

Documents parse each expression once, since `prop()` checks read the parsed params instead of parsing the JSON again. For Rust: `DelegateEvaluable` implementors provide `for_each_expression` rather than `has_expressions`, and `Expression` has `reads`, found once when it is built.
