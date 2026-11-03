---
"@lumiscia/lumen-bindings": minor
---

Expressions no longer treat numeric strings as numbers. `'1' + 2`, `vec2('12.5', 2)` and a param set to `='12'` now fail with an error naming the string instead of converting it; a number still becomes a string for text content, and booleans still count as 0 and 1. Only exact zero is zero: `1 / sin(3.141592653589793)` is a large number rather than a division-by-zero failure, `mod` and `%` behave the same way, and a value below 2.2e-16 now counts as true. `text_width(node(id), 32)` is rejected when the composition loads instead of failing mid-render.

For Rust: `Expression::new` is no longer public, and `lumen-bench-expression` times expression parsing and evaluation on their own.
