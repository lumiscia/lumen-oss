# lumen-server-api

`lumen-server-api` is the HTTP API of `lumen-server` as Rust types: the request and response bodies of every route, the render socket's events, and the error body with its `ErrorCode`. The server (de)serialises with them, and `lumen-generators` emits [`definitions/api.schema.json`](../../definitions/api.schema.json) from them, so another implementation of the API can follow the same contract.

Every object on the wire is camelCase. A request's composition is a `serde_json::Value` whose schema is `composition.schema.json`, and the values the engine produces (diagnostics, media failures, model info) are `Value`s whose schemas are in `runtime.schema.json`; the API schema refers to both by `$ref`. The crate depends only on `serde` and `serde_json`, so it builds without FFmpeg or a GPU.

This crate is experimental: the API can change between releases.

## Features

- `metadata` derives the JSON Schema the definitions are generated from (`schemars`).

## Development

```bash
cargo test -p lumen-server-api
just generate-definitions
```
