---
"@lumiscia/lumen-bindings": patch
---

A render that fails before its first frame reaches a Vulkan encoder now reports its error instead of crashing the process. `lumen-server` crashed on this with `h264_vulkan`, its Linux default, and took every concurrent job with it, for example on a composition whose video `source` isn't staged.
