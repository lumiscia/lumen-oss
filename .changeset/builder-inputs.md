---
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-types": minor
---

`addNode` takes `inputs`, so a node connects to the nodes that feed it where it is added: `addNode({ type: "merge", inputs: { base: background, overlay: title } })`. A variadic port such as `layers` takes an array, `{ layers: [background, image, title] }`. Ports are typed from the node kind, and the new generated `NodeVariadicInputPortName<Kind>` marks the ones that take arrays. The connections are the ordinary ones, in the order written, so the JSON is the same as with `connect`, which stays. `literal(text)` escapes a leading `=` as `==`, and `expr("if(frame > 30, 1, 0)")` wraps raw expression text, such as `if(..)` and `node(..)`, as an `Expr`.

Breaking: `Composition` loses `setSize`, `setFps`, `setDurationFrames`, `setDurationSeconds`, `setMetadata` and `setAudio`; pass `renderSettings`, `timeline` and `metadata` to the constructor. `connect` loses `fromPort`, which the engine only accepts as `output`. `connect` and `inputs` throw for a node that isn't from this composition, and a failed `addNode` of a clip leaves no template or function behind. A composition definition no longer has `.composition`. `moves(base, [{ by, .. }])` emits `by * tween(0, 1, ..)` instead of `tween(0, by, ..)`, with the same value, and `Expr` no longer tracks a kind. `fn` and `addFunction` no longer check names and `wgslShader` no longer checks uniform names or types; the engine reports them when the composition loads.
