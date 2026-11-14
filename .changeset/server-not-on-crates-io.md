---
"@lumiscia/lumen-bindings": minor
---

`lumen-server` is no longer published to crates.io, so `cargo install lumen-server` stops at 0.5. Build it from the repository (`cargo run -p lumen-server --release`); container images for self-hosting will follow. The crate that holds its HTTP API types, `lumen-server-api`, is not published either: other implementations of the API use `definitions/openapi.json` or the generated types in `@lumiscia/lumen-types`.
