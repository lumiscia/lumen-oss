---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-preview": patch
---

The WebGPU preview no longer freezes for seconds on compositions whose frames the GPU can't finish in a frame's time, or that use many intermediate textures.

- `renderNow`, and so a playing preview, resolves once the GPU has finished the frame instead of as soon as it is submitted. Playback follows the GPU's pace and skips frames when it falls behind, where before the browser queued the work and then blocked. `renderMs` in `onStats` now includes the GPU's time.
- Intermediate textures are render attachments, so a browser zeroes a new one on the GPU instead of uploading a buffer of zeros for it.
- The `pooled_textures` budget of `CacheBudgets` counts only the textures the latest frame left unused. It used to count retained outputs and the frame's own textures too, so a composition holding more than the budget in those kept no spares and created its intermediate textures again every frame. The renderer can now hold up to that budget more than before.
