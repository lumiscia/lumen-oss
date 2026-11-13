---
"@lumiscia/lumen-bindings": patch
---

Reports the time a render waits for the GPU as `gpu_ms` in the `lumen` summary line, instead of counting it under `readback_ms`. `readback_ms` is now only the copy of finished frames to the CPU, and `flush_ms` only the collection of finished work, which no longer includes the waits that `gpu_ms` holds. The server's `RenderTiming` gains a matching `gpu` field.
