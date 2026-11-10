# Lumen

Lumen is the open-source rendering core behind [Lumiscia](https://lumiscia.com), a hosted platform for GPU-rendered, node-based media composition.

If you want to use Lumen for real work today, the easiest path is the hosted platform: [lumiscia.com](https://lumiscia.com). It packages Lumen with managed GPU rendering, storage, collaboration, and the operational pieces needed to run renders reliably.

This repository is a heavy-beta, open-source extraction of the engine, runtime, schemas, TypeScript packages, and local tooling. APIs are still moving quickly. Most crates should be treated as experimental and may change as the renderer and hosted platform evolve.

## Quickstart

Build a composition, render it on the hosted API, and preview it in the browser. A self-hosted `lumen-server` takes the same payload; see [`crates/server`](crates/server/README.md).

```bash
pnpm add @lumiscia/lumen-sdk
```

`composition.ts`:

```ts
import { Composition } from "@lumiscia/lumen-sdk";

export const composition = new Composition({ timeline: { durationSeconds: 3 } });
const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
const title = composition.addNode({
  type: "text",
  params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
});
const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
composition.addNode({ type: "media_output", inputs: { source: merged } });
```

Render it, wait for the result, and print the URL of the MP4:

```ts
import { Lumen } from "@lumiscia/lumen-sdk";
import { composition } from "./composition.js";

const lumen = new Lumen({ apiKey: process.env.LUMISCIA_API_KEY! });
const render = await lumen.render(composition);
console.log((await lumen.waitForRender(render)).url);
```

Preview it in a React page with `pnpm add @lumiscia/lumen-react`. The preview needs a browser with WebGPU. Svelte has the same `Player` in `@lumiscia/lumen-svelte`.

```tsx
import { Player, createLumenPreview } from "@lumiscia/lumen-react";
import { composition } from "./composition.js";

const preview = createLumenPreview();

export const App = () => (
  <>
    <Player preview={preview} composition={composition} style={{ width: 640 }} />
    <button onClick={() => preview.play()}>Play</button>
  </>
);
```

See the [documentation](https://github.com/lumiscia/docs) for nodes, media, audio and fonts, and [`examples/`](examples/README.md) for complete apps.

## Platform Support

Lumen requires a GPU for the main renderer paths.

The supported native targets today are:

- Linux with Vulkan rendering, Vulkan Video encoding (`h264_vulkan`, `hevc_vulkan`, `av1_vulkan`) and optional CUDA/NVENC interop for hardware video paths.
- macOS with Metal rendering and VideoToolbox encoding paths.

Other platforms may compile in pieces, especially schema/type generation or pure data crates, but they are not supported render targets yet. Windows is a basic development target for TypeScript packages and pure Rust crates, not a supported native renderer target; see [docs/windows.md](docs/windows.md) for the covered paths and known gaps.

See [docs/support-matrix.md](docs/support-matrix.md) for the current operating system, GPU backend, adapter, encoder, and CI coverage matrix.

## Hosted vs Self-Hosted

The intended production path is the hosted Lumiscia platform:

- managed GPU rendering
- hosted storage and artifact delivery
- product UI and collaboration workflows
- infrastructure maintained by the Lumiscia team

Self-hosting is possible, but still hands-on. The hosted platform is the smoother production path today.

## Self Hosting

The self-hosted entry point is `crates/server`. It provides the render library plus a small local HTTP server that can accept hosted-style render payloads, download direct `http`/`https` media URLs, run a local render, expose progress, and return an MP4 artifact.

For anything beyond local or single-process usage, expect to provide the surrounding infrastructure yourself: GPU machines, FFmpeg/runtime setup, queueing, durable artifact storage, API/auth layers, deployment, monitoring, and provider-specific worker logic.

See [`crates/server`](crates/server/README.md) for the current self-hosted HTTP API, configuration, and limitations.

## What's Included

- `crates/engine`: core composition model, nodes, media handling, and GPU render orchestration
- `crates/gpu`: lower-level `wgpu` renderer resources, programs, passes, and Vulkan export helpers
- `crates/ffmpeg`: FFmpeg bindings for decode, encode, muxing, and GPU frame interop
- `crates/server`: embeddable render library and a small local HTTP binary
- `crates/local`: local render/debug CLI
- `crates/bench`: benchmark binaries for composition and decode paths
- `crates/generators`: schema and metadata generators
- `crates/macros`: internal proc macros for node metadata
- `crates/text`: text layout and glyph data helpers
- `crates/3d`: mesh, transform, camera and draw-order maths for the 3D scene nodes
- `definitions`: generated node metadata and JSON schemas
- `packages/lumen-types`: generated TypeScript types for schemas and metadata
- `packages/lumen-shared`: dependency-light composition builder
- `packages/lumen-sdk`: client for the hosted Lumiscia API (upload media, render, wait, download), re-exporting the composition builder
- `packages/lumen-preview`: browser preview engine and media bridge
- `packages/lumen-bindings`: the preview renderer compiled to WebAssembly, with the preview worker and `/browser`, `/bundler` and `/node` entry points
- `packages/lumen-react` and `packages/lumen-svelte`: the `Player` component for each framework
- `examples/vite-react` and `examples/vite-svelte`: preview a video with sound and a title
- `examples/node`: renders the same composition through the hosted API

Product application code, hosted-service API code, billing/auth flows, and private frontend work live outside this open-source renderer repository. Project documentation lives in the public [lumiscia/docs](https://github.com/lumiscia/docs) repository.

## Development

Install JavaScript dependencies:

```bash
pnpm install
```

Run checks and tests. On a clean checkout, generate the WebAssembly bindings first (see below). Packages resolve each other through their built output, so build before checking:

```bash
just wasm-bindings-debug
pnpm build
pnpm check
pnpm test
cargo check
cargo test
```

Run Rust checks for common native targets:

```bash
cargo check -p lumen-server --features vulkan,cuda --lib
cargo check -p lumen-server --features metal --lib
```

Build generated bindings and metadata:

```bash
just wasm-bindings-debug
just generate-definitions
just generate-types
```

`just wasm-bindings-debug` and `just wasm-bindings-release` compile Rust to WebAssembly and write the bindings into `packages/lumen-bindings/wasm`, which is not committed. A clean checkout needs one of them before `lumen-bindings` builds or an example runs, and it can take a while. Use `debug` while developing and `release` for a size-optimised build. They need the `wasm32-unknown-unknown` Rust target and `wasm-bindgen-cli` at the version pinned in `crates/wasm/Cargo.toml`.

## Status

Lumen is open source, but still early. Expect breaking changes in crate APIs, schema shapes, node behavior, render internals, and package structure while the project settles.

Text nodes use `render_mode: "raster"` by default. Set `"msdf"` to reuse CPU-generated outline distance fields across sizes; colour, bitmap, and overlapping glyphs use raster.
