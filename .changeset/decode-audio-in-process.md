---
"@lumiscia/lumen-bindings": minor
---

Native audio decodes inside the renderer, so `lumen-local` and `lumen-server` no longer need the `ffmpeg` executable on `PATH`, and a long audio track no longer keeps every decoded 30 s window in memory: a 20 minute two-clip render peaked at 1035 MiB and now peaks at 177 MiB. Audio that starts after the container's first timestamp, such as a video whose audio track begins half a second late, now reads as silence until it begins, where the executable played it from the start. A source that ends before a clip does returns the samples it has, where it was padded with silence to the requested length.

For Rust: `lumen-engine` drops its `ffmpeg-next` dependency, and `lumen-engine-ffmpeg` gains `AudioRangeReader`, which reads 48 kHz stereo ranges of an audio stream.
