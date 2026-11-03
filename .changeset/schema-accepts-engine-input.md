---
"@lumiscia/lumen-bindings": minor
---

The composition schema no longer rejects numbers outside a param's slider range, such as `opacity: 2`; the engine clamps them as it always has, and the ranges stay in `x-lumen-constraints`. The schema now also rejects negative node IDs and audio clips without a start and a duration, which the engine already refused. A node whose `params` isn't an object fails with the type it got.
