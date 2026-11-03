---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-types": minor
---

Expressions fail earlier and accept more number forms. An unknown variable such as `=frmae + 1` is now rejected when the composition loads instead of failing mid-render, an unknown string escape such as `'\q'` is an error rather than dropping the backslash, and `node(id)` rejects ids above 2^53. Numbers may now use exponents and a leading dot (`1e3`, `1.5e-3`, `.5`), and a number too large to represent is rejected rather than becoming infinity. The SDK's helpers write large and small numbers in exponent form as a result.

The `linear(a, b, t)` builtin and the SDK's `linear` helper are gone; use `lerp`. `'linear'` stays as an easing name. `x-lumen-expressions` in the schema now lists `forms` (`if` and `node`) and `reserved` names, which `@lumiscia/lumen-types` exports as `expressionForms` and `expressionReservedNames`.
