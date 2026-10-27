---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

The GPU renderer now renders unchanging parts of a composition once and reuses them: a node whose params hold no expression and that reads no media or shader time, fed only by such nodes, is neither bound nor dispatched again on later frames of its plan. Output frames are unchanged. Kept output lives in the plan's own textures, so it counts against the plan cache budget and is dropped with its plan, and recompiles, font loads and texture replacements render it again.

`memo` now caches. Its `cache_id` and `allow_expressions` params are removed in favour of `key`, an expression evaluated at the frame being rendered: while it gives the same value as on the frame `memo` last rendered, `memo` shows that output and nothing only it reads is bound or dispatched. `"key": "=floor(frame / 4)"` runs a heavy shader on one frame in four, and `"=floor(frame / 2)"` animates on twos. What `memo` holds is the source as rendered on the first frame of each key value that is rendered, so pair it with a `time_remap` to `floor(frame / 4) * 4` when frames may render out of order. A literal or missing `key` adds nothing to the caching above. From TypeScript, `composition.addNode({ type: "memo", params: { key: floor(frame.div(4)) } })`.

`GpuCompositionRenderer::cached_pass_count` and `submitted_pass_count` report how many passes were skipped and submitted, and `lumen-local` prints the per-frame averages as `static_cached` and `passes`.
