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

Generated definitions and migrations should not be edited by hand. Use the generator commands in the `justfile`.

WASM bindings are generated into `packages/lumen-bindings/src` with:

```bash
just release
```

This command compiles the Rust WASM crate and can take a long time on a clean checkout. Run it when working on WASM-facing code, binding exports, preview examples, or release artifacts. Do not add handwritten binding stubs to make local TypeScript checks pass.

## Scope

This repository contains the open renderer, schema, TypeScript package, and self-hostable runtime pieces. Hosted platform features such as billing, team auth, dashboards, and managed storage are intentionally outside this repository.
