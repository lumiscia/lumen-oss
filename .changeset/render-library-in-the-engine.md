---
"@lumiscia/lumen-bindings": minor
---

The render library is now `lumen_engine::render`, behind the engine's new `render` feature: compositions to MP4, MOV, WebM, stills and image sequences on a GPU with FFmpeg, with the disk media store (`LocalMediaStore`) and the font catalog (`FontCatalog`). `lumen-server` is only the HTTP server: a binary with no library and no `cli` feature, so `cargo install lumen-server` and `cargo run -p lumen-server` build it as they are.

An embedder depends on the engine instead of the server:

```toml
# Before
lumen-server = { version = "0.6", features = ["vulkan"] }
# Now
lumen-engine = { version = "0.6", features = ["render", "vulkan"] }
```

```rust
// Before
use lumen_server::render::{RenderOptions, ServerRenderError, render_project};
// Now
use lumen_engine::render::{self, RenderOptions, render_project};
fn failed(error: &render::Error) { /* … */ }
```

The `render` feature turns on `ffmpeg`, `image` and `3d`; the compressed 3D formats (`3d-draco`, `3d-ktx2`, `3d-meshopt`, `3d-webp`) and `vulkan`, `metal` and `cuda` stay opt-in. Every item keeps its name except:

| Before                                            | Now                                           |
| ------------------------------------------------- | --------------------------------------------- |
| `lumen_server::render::*`                         | `lumen_engine::render::*`                     |
| `ServerRenderError`                               | `render::Error`                               |
| `From<ErrorCode> for lumen_server_api::ErrorCode` | removed; `lumen-server` maps the codes itself |
| `cargo run -p lumen-server --features cli`        | `cargo run -p lumen-server`                   |

`lumen_engine::render::OutputFormat` and `FrameRange` are the engine's own types now, with the same JSON as the `lumen-server-api` types of the same names, which stay as they are. The engine's `OutputFormat` is not `#[non_exhaustive]`.

The render library logs under `lumen_engine::render`, so `RUST_LOG=lumen_server=debug` becomes `RUST_LOG=lumen_server=debug,lumen_engine::render=debug` to see a render's own logs. `lumen-server` still logs them at `info` by default.
