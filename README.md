# Lumen

Lumen is a node-based media rendering engine with Rust renderer crates, WebAssembly bindings, and small framework packages for browser previews.

This repository is an open-source extraction of the engine, runtime, package, and documentation history. Product application code, hosted-service API code, billing/auth flows, and private frontend work are intentionally excluded.

## What's Included

- `crates/lumen`: core composition, node, media, and rendering logic
- `crates/lumen-gpu`: GPU render planning and resource management
- `crates/lumen-wasm`: WebAssembly-facing renderer bindings
- `crates/lumen-server`: native render worker/server components
- `packages/lumen-wasm`: browser media bridge and wasm package surface
- `packages/lumen-react` and `packages/lumen-svelte`: preview components
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
```
