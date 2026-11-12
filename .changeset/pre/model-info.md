---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-types": minor
---

A model's animation clips and materials can be listed, so `animation` and `texture_material` no longer have to be guessed from an error.

An uploaded model's media object has `kind: "model"` and a `metadata` listing them, read once when the upload completes and returned by `GET /media/:id`:

```json
{
  "kind": "model",
  "metadata": {
    "animations": [
      { "index": 0, "name": "Walk", "duration": 0.7 },
      { "index": 1, "duration": 2 }
    ],
    "materials": [{ "index": 0, "name": "screen", "hasBaseColorTexture": false }],
    "cameras": []
  }
}
```

In the browser, `modelInfo` on the bindings reads a `.glb` and returns the same lists, throwing on a file a render could not read:

```ts
const { modelInfo } = await createLumenBindings().preview();
const { animations, materials } = modelInfo(new Uint8Array(await file.arrayBuffer()));
```

`TemporaryMedia` has `kind` and `metadata` (`MediaKind`, `MediaMetadata`), `@lumiscia/lumen-types` has `LumenModelInfo`, and `uploadMedia` knows `.glb` file names. The error for an `animation` or `texture_material` the model lacks now lists at most 20 names, then how many more there are.
