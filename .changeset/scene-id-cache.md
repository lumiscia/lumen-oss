---
"@lumiscia/lumen-bindings": patch
---

Parsing a composition again no longer discards what the renderer cached for it. Scenes were
identified by where the parsed graph sat in memory, so a second parse of the same document
rendered from a cold cache; they are now identified by an id taken from the graph itself.
