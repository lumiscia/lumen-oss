---
"@lumiscia/lumen-bindings": minor
---

`lumen-server` keeps `h264_vulkan` frames on the GPU, and `lumen-local` gains prefetch, text sessions and CUDA, because both now run one render loop. For Rust: `lumen_server::render` adds `Mp4Render`, and `LocalMediaStore::new` takes the media root and font directory.
