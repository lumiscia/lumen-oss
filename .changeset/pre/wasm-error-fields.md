---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
---

Every engine failure that leaves the WASM bindings is now an `Error` with the same fields, and a failure of `audioTimeline` or `frameRequirements` is no longer a string. An error carries `code`, the engine's diagnostic code; `media` when a file is the cause; `diagnostics` when a composition doesn't parse; and `frameNotLoaded: true` for a video frame that isn't loaded yet, which now keeps its real code, `media.frame_out_of_range`, instead of `code: "media_frame_out_of_range"`. Failures from a caller's mistake, such as a buffer of the wrong length, and from the GPU stay plain strings.

`frameRequirements` and `audioTimeline` return the JSON of the engine's `FrameRequirements` and `AudioPlan`, as typed in `@lumiscia/lumen-types`. `frameRequirements` is unchanged. `audioTimeline` no longer has `durationFrames` and `fps`: read them from `durationFrames()` and `fps()` of the controller.

```ts
// Before
try {
  controller.audioTimeline();
} catch (error) {
  console.error(error); // a string, with no code
}
// A frame that isn't loaded yet
if (error.code === "media_frame_out_of_range") {
  /* load it and render again */
}

// After
try {
  controller.audioTimeline();
} catch (error) {
  console.error((error as Error).message, (error as { code?: string }).code);
}
if ((error as { frameNotLoaded?: boolean }).frameNotLoaded) {
  /* load it and render again */
}
```
