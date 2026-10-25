---
"@lumiscia/lumen-bindings": patch
---

Reject lossy audio volume and integer values with errors that name the field, report which part of an invalid color was wrong, and drop the unused `x-lumen-nodeKinds` schema key. Native GPU instances now discard HAL debug labels by default to avoid a Vulkan loader crash with concurrent devices; set `WGPU_DISCARD_HAL_LABELS=0` to keep resource names in GPU captures.
