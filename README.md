# Lumen

Lumen is a node-based media rendering engine with Rust renderer crates, WebAssembly bindings, schema-derived TypeScript packages, and framework preview components.

This repository is an open-source extraction of the engine, runtime, package, and documentation history. Product application code, hosted-service API code, billing/auth flows, and private frontend work are intentionally excluded.

## What's Included

- `crates/lumen`: core composition, node, media, and rendering logic
- `crates/lumen-gpu`: GPU render planning and resource management
- `crates/lumen-wasm`: WebAssembly-facing renderer bindings
- `crates/lumen-server`: native render worker/server components
- `definitions`: generated node metadata and JSON schemas
- `packages/lumen-types`: generated TypeScript types for schemas and metadata
- `packages/lumen-shared`: dependency-light composition helpers
- `packages/lumen-preview`: browser preview engine and media bridge
- `packages/lumen-bindings`: generated WASM binding package surface with the compiled WASM included
- `packages/lumen-react` and `packages/lumen-svelte`: framework preview wrappers
- `examples/vite-react` and `examples/vite-svelte`: local preview examples
- `docs` and `specs`: architecture notes, render specs, and migration plans

## Development

Install dependencies with Vite+:

```bash
vp install
```

Run checks and tests:

```bash
vp check
vp test
cargo check
cargo test
```

Build generated bindings and metadata:

```bash
just release
just generate-node-specs
just generate-definitions
vp run generate:types
```

`just release` writes the generated WASM bindings directly into `packages/lumen-bindings/src`, so the package can be published without a separate download step. WASM release artifacts are also staged for GitHub Releases with:

```bash
just release-artifacts
```
