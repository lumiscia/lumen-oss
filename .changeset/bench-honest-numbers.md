---
"@lumiscia/lumen-bindings": minor
---

`lumen-bench-composition` now reports setup, first frame and steady-state fps separately, prints whether each row ran with static-output reuse disabled, and gains a `vulkan-video` mode that renders and encodes on one Vulkan device. The `*-profile` modes became `--profile`, and the `vk-cuda-export` mode and `vector_showcase` composition are removed; `--save` no longer overwrites one file when several encodes run. `lumen-bench-decode` drops its memory columns and fails if the crate and the ffmpeg CLI decode different amounts of video, and `lumen-bench-text` skips the emoji case when no loaded font covers it.
