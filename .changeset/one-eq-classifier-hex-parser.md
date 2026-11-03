---
"@lumiscia/lumen-bindings": patch
---

Every place that reads a leading `=` or a hex colour now shares one implementation. A `color` prop on a composition template no longer accepts `#+f+f+f`, which only that path let through; it is now an error, as it already was everywhere else. Other input parses as before.
