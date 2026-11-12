---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

The browser preview draws `model_3d`. Pass each model's `.glb` file in `media`, under the name its `source` uses, as you would an image:

```ts
const fox = composition.addNode({
  type: "model_3d",
  params: { source: "fox", animation: "Walk", scale: [0.01, 0.01, 0.01] },
});
// ...wire `fox` into a merge_3d or render_3d as usual.

mountPreview(host, { preview, composition, media: { fox: "/models/fox.glb" }, wasmUrl, workerUrl });
```

A name that a `model_3d` uses is read as a model, and so is a blob of type `model/gltf-binary`; a source that only an expression names needs `{ kind: "model", source }` when neither its name nor its blob's type says. An `asset:<id>` source is a `media` name like any other. A `model_3d` or `media_in` inside a `compositions` template now says what its `media` entry holds just as one at the root does, so an image a template reads no longer needs a `kind` either.

Models load with the composition, as images do: the preview finishes loading, and draws its first frame, once every model has arrived, and until then keeps showing what it showed before. A model that can't be fetched fails the load with `load_failed`, naming the source. A file that isn't a model Lumen can read fails the load with the `media.*` code a render would give, before anything is drawn. A model left out of `media` is released, along with its GPU copy.

The preview now parses the composition before it fetches any media, so a composition the engine rejects fails at once instead of after its media has loaded. A preview built without 3D support reports `node.unsupported` for a `model_3d` without fetching its file.
