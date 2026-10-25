# Contributing

Thanks for helping improve Lumen.

## Development

Install dependencies:

```bash
pnpm install
```

Before opening a pull request, run:

```bash
pnpm check
pnpm test
cargo fmt
cargo check
cargo test
```

Windows development has basic CI coverage for TypeScript package builds and pure Rust crates that do not require local FFmpeg or renderer system libraries. Native renderer/server development is still Linux/macOS first; see [`docs/windows.md`](docs/windows.md) for the current Windows setup path and known gaps.

Generated definitions and migrations should not be edited by hand. Use the generator commands in the `justfile`. CI runs `just verify-definitions` to make sure committed files in `definitions/` match the Rust node definitions.

WASM bindings are generated into `packages/lumen-bindings/src` with:

```bash
just wasm-bindings-debug
```

This command compiles the Rust WASM crate for local development and can take a long time on a clean checkout. Run it when working on WASM-facing code, binding exports, or preview examples. Do not add handwritten binding stubs to make local TypeScript checks pass.

## Scope

This repository contains the open renderer, schema, TypeScript package, and self-hostable runtime pieces. Hosted platform features such as billing, team auth, dashboards, and managed storage are intentionally outside this repository.
