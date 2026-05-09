# Lumen

Lumen is the open-source rendering core behind [Lumiscia](https://lumiscia.com), a hosted platform for GPU-rendered, node-based media composition.

If you want to use Lumen for real work today, the easiest path is the hosted platform: [lumiscia.com](https://lumiscia.com). It packages Lumen with managed GPU rendering, storage, collaboration, and the operational pieces needed to run renders reliably.

This repository is a heavy-beta, open-source extraction of the engine, runtime, schemas, TypeScript packages, and local tooling. APIs are still moving quickly. Most crates should be treated as experimental and may change as the renderer and hosted platform evolve.

## Platform Support

Lumen requires a GPU for the main renderer paths.

The supported native targets today are:

- Linux with Vulkan rendering and optional CUDA/NVENC interop for hardware video paths.
- macOS with Metal rendering and VideoToolbox encoding paths.

Other platforms may compile in pieces, especially schema/type generation or pure data crates, but they are not supported render targets yet. Windows support is not currently a target of this open-source tree.

## Hosted vs Self-Hosted

The intended production path is the hosted Lumiscia platform:

- managed GPU rendering
- hosted storage and artifact delivery
- product UI and collaboration workflows
- infrastructure maintained by the Lumiscia team

Self-hosting is possible, but still hands-on. The hosted platform is the smoother production path today.

## Self Hosting

The self-hosted entry point is `crates/lumen-server`. It provides render service primitives plus a small local HTTP server that can accept hosted-style render payloads, download direct `http`/`https` media URLs, run a local render, expose progress, and return an MP4 artifact.

For anything beyond local or single-process usage, expect to provide the surrounding infrastructure yourself: GPU machines, FFmpeg/runtime setup, queueing, durable artifact storage, API/auth layers, deployment, monitoring, and provider-specific worker logic.

See [`crates/lumen-server`](crates/lumen-server/README.md) for the current self-hosted HTTP API, configuration, and limitations.

## What's Included

- `crates/lumen`: core composition model, nodes, media handling, and GPU render orchestration
- `crates/lumen-gpu`: lower-level `wgpu` renderer resources, programs, passes, and Vulkan export helpers
- `crates/lumen-ffmpeg`: FFmpeg bindings for decode, encode, muxing, and GPU frame interop
- `crates/lumen-server`: embeddable render service primitives and a small local HTTP binary
- `crates/lumen-wasm`: WebAssembly bindings for browser preview/render integration
- `crates/lumen-local`: local render/debug CLI
- `crates/lumen-bench`: benchmark binaries for composition and decode paths
- `crates/lumen-generators`: schema and metadata generators
- `crates/lumen-macros`: internal proc macros for node metadata
- `crates/lumen-text`: text layout and glyph data helpers
- `definitions`: generated node metadata and JSON schemas
- `packages/lumen-types`: generated TypeScript types for schemas and metadata
- `packages/lumen-shared`: dependency-light composition helpers
- `packages/lumen-preview`: browser preview engine and media bridge
- `packages/lumen-bindings`: generated WASM binding package with compiled WASM included
- `packages/lumen-react` and `packages/lumen-svelte`: framework preview wrappers
- `examples/vite-react` and `examples/vite-svelte`: local preview examples
- `docs` and `specs`: architecture notes, render specs, and migration plans

Product application code, hosted-service API code, billing/auth flows, and private frontend work live outside this open-source renderer repository.

## Development

Install JavaScript dependencies:

```bash
pnpm install
```

Run checks and tests:

```bash
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
just release
just generate-definitions
pnpm generate:types
```

`just release` compiles Rust to WebAssembly and writes generated bindings directly into `packages/lumen-bindings/src`, so it can take a while. The package can then be published without a separate download step. WASM release artifacts are also staged for GitHub Releases with:

```bash
just release-artifacts
```

## Status

Lumen is open source, but still early. Expect breaking changes in crate APIs, schema shapes, node behavior, render internals, and package structure while the project settles.
