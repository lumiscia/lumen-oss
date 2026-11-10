---
"@lumiscia/lumen-bindings": patch
---

`lumen-server` no longer parses and checks a composition on the async runtime when a render is created, and concurrent renders of the same media now share one resolver instead of each keeping its own.
