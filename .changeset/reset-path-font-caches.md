---
"@lumiscia/lumen-bindings": patch
---

A renderer reused for another composition no longer keeps the previous one's path and font caches, and video frames that already match the canvas size are uploaded without an extra copy.
