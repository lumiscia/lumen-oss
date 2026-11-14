---
"@lumiscia/lumen-types": minor
---

**Each entry of `diagnosticCodes` says where its code is found and what it means.** An entry is now `{ code, severity, foundBy, meaning }`: `foundBy` is `"check"`, `"render"` or `"check, render"`, and `meaning` is a sentence of Markdown. Each member of `DiagnosticCode` carries the same sentence as its doc comment, so an editor shows it on hover.

```ts
import { diagnosticCodes } from "@lumiscia/lumen-types";

const entry = diagnosticCodes.find(({ code }) => code === "node.unknown_param");
// { code: "node.unknown_param", severity: "error", foundBy: "check",
//   meaning: "A node has a param its type doesn't." }
```
