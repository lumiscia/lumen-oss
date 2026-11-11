---
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-sdk": minor
---

Expressions gain typed conditions. `Expr` has `gt`, `gte`, `lt`, `lte`, `eq` and `neq`, which evaluate to `Expr<boolean>`, and `and`, `or` and `not` on booleans. `when(condition, then, otherwise)` builds `if(..)` for numbers, vec2 values, colours and strings, and `node(id, 'property')` reads another node's param.
