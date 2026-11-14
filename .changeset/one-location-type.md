---
"@lumiscia/lumen-bindings": minor
---

Errors and warnings say where they happen in one way. Diagnostic codes and the shape of a diagnostic are unchanged.

- A warning a render raises, such as `font.fallback` or `media.image_reduced`, now has its node's pointer as its `path`, such as `/nodes/3` or `/compositions/card/nodes/0`. It used to have only the root's or the template's. A render raises at most 32 warnings per composition, and `media.image_reduced` warns once for each node that shows the image, where it used to warn once per image.
- Evaluation errors spell the node's kind as the document's `type` does: `render_3d node 4 evaluation failed: …`, `model_3d node 1 …`, `clip node 2 …`, where some read `Render3d node 4`.
- Some messages put the place first: ``node 1: unknown param `paintt` ``, `node 3: duplicate node id`, ``node 2 `color`: invalid type: expected a solid color, got a gradient``, ``node 4: input `source` takes a raster, but node 1 outputs a scene: …``. A media failure reads ``image `big.png` (composition `card`: node 3 `source`) is too large``, with a colon where it had a comma.

For Rust embedders, `lumen_engine::error::At { template, node, param }` is the one location type. The variants that held `node_id` or `template` fields hold `at: At`, `GraphValidationError::InTemplate` is gone (the template is in `at`), and `Warning` is a struct with `at` and `kind: WarningKind`:

```rust
// Before
Warning::FontFallback { template: None, node_id: NodeId::new(3), family: "Brand".into() }
// After
Warning {
    at: At::node(NodeId::new(3)),
    kind: WarningKind::FontFallback { family: "Brand".into() },
}
```
