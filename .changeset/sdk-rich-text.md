---
"@lumiscia/lumen-shared": minor
---

`Composition.addNode` takes typed `Expr` values inside rich text, not just top-level params: span fields in `content`, and the fields of `underline`, `strikethrough` and `highlight` at node and span level (`{ extent: vec2(0, progress(0, 30)) }`). Any nested field that accepts an `=...` string accepts an `Expr` of the matching type, and the new `ExpressionInput<T>` type applies the same mapping to a value built outside `addNode`, such as `ExpressionInput<TextSpan>`.
