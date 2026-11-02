---
"@lumiscia/lumen-bindings": patch
---

`lumen-server` built with `cuda` and `vulkan` no longer uses hardware video decode by default, because it draws 640x360 and 1920x1080 video in the wrong colours. Set `LUMEN_HARDWARE_DECODE=1` to turn it back on.
