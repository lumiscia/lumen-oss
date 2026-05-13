# lumen-engine-text

`lumen-engine-text` contains text layout and glyph data helpers used by the renderer.

This crate is experimental and currently tracks Lumen's renderer needs rather than a standalone text engine API.

## Platform Notes

This crate itself does not require a GPU, but its output is designed to feed GPU render paths in `lumen`.

## Development

```bash
cargo check -p lumen-engine-text
cargo test -p lumen-engine-text
```
