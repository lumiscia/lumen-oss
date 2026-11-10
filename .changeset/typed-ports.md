---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

Node inputs and outputs have a kind, `raster` or `scene`, so a scene can't be wired where a raster goes or the reverse. Every node today takes and outputs rasters. A connection joining a node's output to an input of another kind fails validation with `connection.wrong_kind`, naming both nodes, the input and the two kinds, and its diagnostic points at the connection. The generated definitions give each port's and output's kind as `raster` or `scene`, where they said `raster_frame`, and list the kinds as `x-lumen-portKinds`; `NodePortKind` is `"raster" | "scene"`, and `NodeOutputKind` and `NodeInputPortKind` give a node kind's output and an input's kind. In the SDK, `addNode` inputs and `connect` only take nodes whose `type` outputs what the port takes. `NodeReference<"scene">` or `NodeReference<"raster">` is a node for a port of that kind, and a plain `NodeReference` is any node; a bare id isn't checked.
