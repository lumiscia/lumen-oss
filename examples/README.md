# Examples

Each example previews or renders a 4 s clip with sound (`assets/clip.mp4`) under a title set in Roboto, which Lumen bundles.

- `vite-react` and `vite-svelte` preview it in the browser, which needs WebGPU.
- `node` renders it through the hosted API and writes `out.mp4`.

From the repository root:

```bash
pnpm install
```

The browser examples also need the WebAssembly bindings, which are generated rather than committed. They need Rust with the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` at the version pinned in `crates/wasm/Cargo.toml`:

```bash
just wasm-bindings-debug
pnpm --filter @lumiscia/example-vite-react dev   # or example-vite-svelte
```

Add `?scene=3d` to the URL for a 3D scene.

`dev` builds the workspace packages first, and stops with that command if the bindings are missing.

```bash
LUMISCIA_API_KEY=... pnpm --filter @lumiscia/example-node start
```

`start` builds the packages and the example, and exits with an error without `LUMISCIA_API_KEY`. `LUMISCIA_BASE_URL` points it at another API. `lumen-server` has no media upload, so it needs a different `media` than the one in `src/index.ts`.

## 3D

`3d` writes three compositions that `lumen-local` renders on a local GPU. See the [3D guide](../docs/3d.md) for the nodes they use.

- `layers`: 2D layers (a gradient, text, hills and a dot) at different depths under a camera that swings and dollies in.
- `animated-model`: the glTF fox running its `Run` clip over a shadow catcher, with an orbiting camera.
- `video-on-model`: `assets/clip.mp4` on the material of a model, a sheet that swings about its vertical axis.

From the repository root:

```bash
pnpm install
pnpm --filter @lumiscia/example-3d start   # writes examples/3d/compositions/*.json
cargo run -p lumen-local -- --composition examples/3d/compositions/animated-model.json --output animated-model.mp4
cargo run -p lumen-local -- --composition examples/3d/compositions/layers.json --output layers.png --frame 60
```

Run `lumen-local` from the repository root: the compositions name their media by paths under it, and `lumen-local` reads media from the directory it runs in unless `--media-root` says otherwise.

`animated-model` uses the glTF sample fox in `crates/engine/tests/fixtures/Fox.glb`, which is CC BY 4.0; its credits are in `Fox.README.md` beside it. `video-on-model` uses `crates/3d/tests/fixtures/grid.glb`, a sheet the model reader's tests generate, licensed like this repository.

`assets/clip.mp4` is a synthetic 640×360 H.264/AAC clip made for this repository and licensed like it (Apache-2.0):

```bash
ffmpeg -f lavfi -i "gradients=s=640x360:d=4:r=30:speed=0.015:c0=0x1d4ed8:c1=0xf97316:c2=0x0f172a:c3=0xe11d48:nb_colors=4" \
  -f lavfi -i "aevalsrc='(0.25*sin(2*PI*t*(220*pow(1.25992,mod(floor(t*2),4))))+0.12*sin(2*PI*t*1.5*(220*pow(1.25992,mod(floor(t*2),4)))))*exp(-3*mod(t,0.5))':s=48000:d=4:c=stereo" \
  -vf "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p" -colorspace bt709 -color_range tv \
  -c:v libx264 -preset veryslow -crf 30 -profile:v main -movflags +faststart -c:a aac -b:a 64k -t 4 assets/clip.mp4
```
