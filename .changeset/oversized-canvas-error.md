---
"@lumiscia/lumen-bindings": patch
---

Rendering a canvas larger than the device's texture limit, including a clip's template that declares one, now returns a render error naming the node and the limit instead of panicking.
