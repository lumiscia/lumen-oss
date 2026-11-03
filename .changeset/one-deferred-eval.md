---
"@lumiscia/lumen-bindings": patch
---

Params evaluate through one `Deferred::eval` that holds the only literal-or-expression match, and each param type now only converts an evaluated `ExpressionValue`. Compositions render and fail as before. For Rust: `DeferredValue` is now `ParamValue`, `eval_deferred` is now `from_expression`, and `eval_number` and `eval_int` are replaced by `truncated_int`, which takes the evaluated value.
