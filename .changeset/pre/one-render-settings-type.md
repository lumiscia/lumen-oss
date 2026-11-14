---
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-bindings": minor
---

**A template's `render_settings` has the same type as the root's.** `TemplateRenderSettings` is gone from `@lumiscia/lumen-types`; use `RenderSettings`.

```ts
// Before
import type { TemplateRenderSettings } from "@lumiscia/lumen-types";

// After
import type { RenderSettings } from "@lumiscia/lumen-types";
```

A document that still carries the long-retired `background_color` in `render_settings`, or `start_frame`, `duration_frames` or `source_start_seconds` on an audio clip, now fails with the ordinary unknown-field error (`render_settings.background_color: unknown field`) instead of a message that says what to use instead. Remove the key; colour the canvas with a `background` node, and give audio clips `start_ms`, `duration_ms` and `source_start_ms`.
