---
"@lumiscia/lumen-bindings": patch
---

`h264_vulkan` renders no longer hang on NVIDIA. The encoder could stall forever at a keyframe, which stopped `lumen-local` mid-render. It now encodes one frame at a time, which adds about 1.3–2.2 ms per 1080p frame, or 20–65% on cheap compositions. B-frames, file size and quality are unchanged.
