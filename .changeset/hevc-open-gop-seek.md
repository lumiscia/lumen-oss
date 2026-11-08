---
"@lumiscia/lumen-bindings": patch
---

Open-GOP HEVC videos, such as x265's default output, showed the next frame instead of the one asked for when playback stepped backwards onto a frame just before a keyframe, on both CPU and hardware decode. A seek whose first decoded frame comes after its target now seeks again from an earlier keyframe.
