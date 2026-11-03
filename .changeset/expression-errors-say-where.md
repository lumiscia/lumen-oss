---
"@lumiscia/lumen-bindings": minor
---

Render errors now say what failed. An expression failure names the node, the param and the variable, as in `node 1 `content[1].font_size`: division by zero` and `node 1 `font_size`: undefined variable `foo``, and one reading another node's param names both. Graph errors name the nodes and ports they hold, and a cycle prints its path. Parse errors give the expression's source and the byte the parser stopped at. Messages no longer repeat themselves or carry an `expression error:` style prefix.

For Rust: `ExpressionContext::path` and `NodeParamEvalContext` are gone, `ExpressionContext` is `Copy`, `DelegateEvalContext` holds only `node_id` and `expr`, `NodeParams` builds on `DelegateEvaluable`, an expression that returns the wrong type now fails with `ExpressionError` rather than `PropertyError::InvalidType`, and `RenderError` variants drop the `frame` field they never printed.
