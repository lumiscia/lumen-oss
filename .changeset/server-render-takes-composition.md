---
"@lumiscia/lumen-bindings": minor
---

`lumen_server::render::render_project_mp4` takes a `&Composition`, and `parse_composition` replaces `convert_project_payload`, `ProjectBundle` and `ProjectInfo`. `RenderOptions::verbose_debug` and `--verbose-debug` are gone; use `RUST_LOG=lumen_server=debug`. A server build without the `cuda` feature renders `*_nvenc` encoders through CPU upload instead of failing, and the synchronous render API no longer needs a Tokio runtime.
