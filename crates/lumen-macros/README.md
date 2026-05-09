# lumen-macros

`lumen-macros` contains internal procedural macros used by Lumen's Rust node definitions.

This crate is experimental and is not intended as a stable public macro API.

## Platform Notes

This crate does not render media and does not require a GPU. It is a compile-time helper for the renderer crates.

## Development

```bash
cargo check -p lumen-macros
```
