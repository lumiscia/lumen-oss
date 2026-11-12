---
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-sdk": minor
---

`expr` can be used as a template tag. ``expr<Vec2>`vec2(0, ${lift})` `` lowers each interpolated value the way builtin arguments are lowered, so built expressions, numbers, strings and tuples go in without stripping a leading `=` by hand. Raw expressions are bracketed where they meet an operator.
