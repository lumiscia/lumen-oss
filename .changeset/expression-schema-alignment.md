---
"@lumiscia/lumen-types": patch
"@lumiscia/lumen-bindings": patch
---

Allow `=` expressions in the schema and TypeScript types exactly where the engine accepts them. Paints, colours, named enums and `alpha_premultiply.mode` no longer type-check with an expression, and number, bool, string and vec2 params that the engine evaluates now do.
