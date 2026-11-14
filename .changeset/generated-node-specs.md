---
"@lumiscia/lumen-types": minor
---

**`nodeSpecs` and `nodeSpecByKind` are exported.** They are generated from the composition schema, so an editor reads each node kind's ports, params, defaults, enum options and limits instead of walking the schema itself. They have the shape of the `NodeSpec` types the package already declared.

```ts
import { nodeSpecByKind, nodeSpecs } from "@lumiscia/lumen-types";

nodeSpecByKind.blur.params[0];
// { id: "radius", name: "Radius", kind: "float", description: "…", defaultValue: 4,
//   constraints: { min: 0, step: 0.5 } }
nodeSpecs.map((spec) => spec.kind); // every node kind, in the schema's order
```
