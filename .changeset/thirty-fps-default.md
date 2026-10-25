---
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-bindings": patch
---

Default new compositions to 30 fps in the TypeScript builder, matching the engine, and convert audio `start_frame`/`duration_frames` at the composition's frame rate instead of a fixed 30. Compositions that relied on the old 24 fps builder default now play faster (240 frames last 8 s instead of 10 s); call `setFps(24)` to keep the old timing.
