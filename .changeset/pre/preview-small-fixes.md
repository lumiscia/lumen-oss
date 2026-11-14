---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-preview": patch
---

Fixes three preview problems. A composition under 1 fps now plays for its real length instead of being timed as if it ran at 1 fps. A `requestInit.signal` on a whole-file media source now aborts the load, together with the preview's own signal. Render failures no longer print to the browser console when logging is off; set a log level with `setLogLevel` to see them.
