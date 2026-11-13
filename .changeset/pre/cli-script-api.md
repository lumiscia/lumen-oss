---
"@lumiscia/lumen-cli": minor
---

`@lumiscia/lumen-cli` can be imported to check, render and preview a composition from a script, so running the script that builds it is all it takes. It works under Node, Bun and Deno, pipes the composition to the installed `lumen` binary and passes the binary's progress and warnings through:

```ts
import { check, preview, render } from "@lumiscia/lumen-cli";

await render(composition, { output: "out.mp4", quality: "high" });
await preview(composition, { output: "sheet.png", count: 20, columns: 5 });
const { ok, diagnostics } = await check(composition);
```

`composition` is the JSON or anything with `toJSON()`, such as the SDK's `Composition`. A failed run rejects with a `LumenCliError` holding the exit code and stderr, and `quiet` and `signal` hide the output and stop the run. `preview()` needs a `lumen` binary with `lumen preview`.
