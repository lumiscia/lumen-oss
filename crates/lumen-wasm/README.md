# lumen-wasm

`lumen-wasm` exposes Lumen renderer bindings for browser environments. It is used to build the generated WASM package consumed by the TypeScript preview packages.

This crate is experimental. Binding names, payload shapes, and browser render behavior may change.

## Platform Notes

This crate targets `wasm32-unknown-unknown` and browser GPU APIs through `web-sys`, `wasm-bindgen`, `lumen`, and `lumen-gpu`.

The native Linux/macOS GPU targets do not apply directly here, but the renderer model is shared with the native crates. Browser support depends on the browser's WebGPU/WebGL support.

## Features

- `json`: enables JSON support through `lumen/json`.
- `embed-roboto`: embeds default Roboto font assets.

## Development

```bash
cargo check -p lumen-wasm --target wasm32-unknown-unknown
just wasm-bindings-release
```

`just wasm-bindings-release` compiles this crate and writes generated package files into `packages/lumen-bindings/src`.
