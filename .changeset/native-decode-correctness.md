---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": patch
---

Rendered output for video inputs changes intentionally: `lumen-local` and `lumen-server` now convert video with its tagged colour matrix and range, where they used to read everything as limited-range BT.601. Untagged video reads as BT.709 from 720 lines up and BT.601 below. Phone video with a rotation flag now renders upright at its display size, native and in the browser preview. Files whose first frame has a non-zero timestamp now show the right frames, and files with several video tracks use the one FFmpeg ranks best. `lumen-local` no longer runs out of memory on long clips, because each video source now caches at most 64 frames or 256 MiB.

Breaking: a video with no usable frame rate or duration now fails to open with `unable to determine stream frame rate` or `unable to determine stream frame count`, where it used to get an invented frame rate or a one-frame length.

For Rust: `VideoStreamInfo` gains `rotation`, `frame_rate` and `start_time`, its `width` and `height` are now display dimensions, and `InputContext::duration_us` is new.
