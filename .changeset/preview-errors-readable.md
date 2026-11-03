---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-bindings": minor
---

Preview errors are readable and precise. `preview.error` is now the message rather than a stack trace, with the stack still logged to the console. Media failures always name their source, through the message and the new `LumenMediaError.sourceId`. Render failures from the WASM bindings are now `Error` objects rather than strings, and a video frame that isn't loaded yet has `code: "media_frame_out_of_range"`. Only that code makes the preview load the frame and render again, so an unrelated error that happens to contain the words "media frame" and "out of range" is no longer retried.
