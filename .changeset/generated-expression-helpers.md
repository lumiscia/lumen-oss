---
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-bindings": minor
---

**Expression helpers show their documentation and argument names on hover.** `abs`, `clamp`, `tween`, `keyframes` and the rest are now generated as one documented function per builtin from the engine's table, instead of being derived from it with conditional types. Hovering a helper shows the table's description and its real parameter names; before, an editor showed `args_0`.

Before, hovering `smoothstep` showed `(args_0: Num, args_1: Num, args_2: Num) => Expr<number>` and no description. After:

```ts
/** Hermite smoothing of `x` between two edges. */
function smoothstep(edge0: Num, edge1: Num, x: Num): Expr<number>;
```

Every call that type-checked before still does, with the same result type, with one loss: an optional argument can be skipped with an explicit `undefined` before a later one, as in `spring(0, undefined, 20)`, without a type error. It still throws at run time, as it did.

The helper types `Builtins`, `CamelCase`, `Interpolate`, `Tween`, `MakeKey` and `Keyframes` are no longer exported; their overloads are part of the functions. `expressionBuiltins` in `@lumiscia/lumen-types` says `"same"` instead of `"any"` for the arguments of `lerp`, `tween` and `key` that must share one type, and for the return of `lerp` and `tween`. The engine accepts and evaluates the same expressions.
