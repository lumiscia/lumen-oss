---
"@lumiscia/lumen-bindings": minor
---

`lumen_server::service` is removed: the queue, executor, artifact store and progress sink traits and their in-memory, presigned-URL and callback implementations had no users. Embed `lumen_server::render` instead. The inert `legacy-decode-tests` feature is gone, and `reqwest` and `futures-util` are only built with the `cli` feature.
