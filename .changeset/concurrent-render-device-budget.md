---
"@lumiscia/lumen-bindings": patch
---

`lumen-server` no longer crashes with several renders running at once on NVIDIA. Renders that would pass the driver's limit on Vulkan contexts wait for a running render to finish.
