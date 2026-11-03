---
"@lumiscia/lumen-bindings": minor
---

Native audio decodes inside the renderer, so `lumen-local` and `lumen-server` no longer need the `ffmpeg` executable on `PATH`, and a long audio track no longer keeps every decoded 30 s window in memory: a 20 minute two-clip render peaked at 1035 MiB and now peaks at 166 to 177 MiB. Audio that starts after the container's first timestamp, such as a video whose audio track begins half a second late, now reads as silence until it begins, where the executable played it from the start. The same holds for a gap in a stream's timestamps, which reads as silence instead of pulling later audio early. A source that ends before a clip does returns the samples it has, where it was padded with silence to the requested length. A damaged packet costs its own samples instead of failing the render, and a stream whose channel layout or sample rate changes midway keeps decoding.

For Rust: `lumen-engine` drops its `ffmpeg-next` dependency, and `lumen-engine-ffmpeg` gains `AudioRangeReader`, which reads 48 kHz stereo ranges of an audio stream.
