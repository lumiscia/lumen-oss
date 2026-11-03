---
"@lumiscia/lumen-bindings": patch
---

Video decoded on the CPU no longer leaves the last columns transparent when the width isn't a multiple of 8. A 854×480 source lost its last 6 columns and a 642-wide one lost 2; 1920×1080 and 1280×720 were unaffected. This affected every host that decodes video with FFmpeg on x86.
