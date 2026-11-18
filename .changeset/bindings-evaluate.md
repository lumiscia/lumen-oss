---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Add `evaluate` to the bindings, which gives the value of one param of a root node at a frame without rendering. It evaluates the param as a render does, so an easing curve, a timing or a `table()` lookup can be checked without reading pixels, and it runs wherever `check` does: no GPU and no media. It returns the value as the JSON that sets the param, and a flattened param such as `position` is addressed by its own name. For an `instance_3d`, `instance` picks the copy whose `position`, `rotation` or `scale` is evaluated.

It throws, with `diagnostics`, on a composition `check` reports errors for, a node or param that doesn't exist, a node inside a template (`render.template_node`, which is new) and an expression that fails, such as one that needs media. `EvaluateRequest` is exported from `@lumiscia/lumen-types`.

```ts
const { evaluate } = await loadLumenBindings();

evaluate(composition, { node: 12, param: "position", frame: 90 });
// [1.5, 0, -3]

evaluate(composition, { node: 4, param: "position", frame: 0, instance: 3 });
```
