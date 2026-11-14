# lumen-generators

`lumen-generators` generates the composition schema from the Rust node definitions, and the runtime schema (diagnostics, the media inventory and the other values the engine returns) from the Rust types that serialise them.

This crate is experimental because the node system, schemas, and generated package contracts are still changing.

## Platform Notes

This crate does not render media and does not require a GPU. It is part of the tooling pipeline that supports the GPU renderer.

## Usage

Most generation should be run through the repository task runner:

```bash
just generate-definitions
just verify-definitions
just generate-types
```

Direct Rust checks:

```bash
cargo check -p lumen-generators
cargo test -p lumen-generators
```
