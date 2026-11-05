---
"@lumiscia/lumen-bindings": patch
---

Seeking in HEVC with B-frames on a hardware decoder no longer returns a frame from before the seek. FFmpeg's HEVC decoder keeps its output queue through a flush when it has no frame threads, so a time remap that stepped back just before an IDR frame drew a later frame instead of the one it asked for. CPU decode had the same bug on a host with one CPU. The same frames now come out on either path.

For Rust: `VideoDecoder::flush` now returns a `Result`.
