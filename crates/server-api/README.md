# lumen-server-api

`lumen-server-api` is the HTTP API of `lumen-server` as Rust: the request and response bodies of every route, the render socket's events, the render webhook, the error body with its `ErrorCode`, and `OPERATIONS`, the table of every route with what it takes, what it answers and which error codes each status carries. The server (de)serialises with the types and builds its router from the table, and `lumen-generators` emits [`definitions/openapi.json`](../../definitions/openapi.json) from both, so another implementation of the API can follow the same contract. [`definitions/README.md`](../../definitions/README.md) says how a host is checked against it.

Every object on the wire is camelCase. A request's composition is a `serde_json::Value` whose schema is `composition.schema.json`, and the values the engine produces (diagnostics, media failures, model info) are `Value`s whose schemas are in `runtime.schema.json`; the OpenAPI document refers to both by `$ref`. The crate depends only on `serde` and `serde_json`, so it builds without FFmpeg or a GPU.

This crate is experimental: the API can change between releases.

## Features

- `metadata` derives the JSON Schemas the OpenAPI document is generated from (`schemars`).

## Development

```bash
cargo test -p lumen-server-api
just generate-definitions
```
