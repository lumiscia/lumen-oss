# lumen-server

`lumen-server` provides provider-neutral rendering service primitives for Lumen plus a small local HTTP binary.

For production usage, the smoothest path is the hosted platform at [lumiscia.com](https://lumiscia.com). Self-hosting this crate is possible, but you will still need GPU machines, queueing, artifact storage, auth, deployment, monitoring, and provider-specific worker logic.

This crate is experimental and its service traits may change.

## Platform Notes

Native rendering requires a GPU.

Supported targets today are:

- Linux with Vulkan rendering and optional CUDA/NVENC interop when built with `vulkan,cuda`.
- macOS with Metal rendering and VideoToolbox-oriented media paths when built with `metal`.

The crate does not include RunPod, Vast, Cloudflare, billing, accounts, hosted auth, or deployment automation.

## Targets

`lumen-server` ships both:

- a library target, `lumen_server`, for embedding in custom render platforms
- a binary target, `lumen-server`, for local HTTP rendering

The exposed Axum API is intentionally binary-owned. Its request shape supports remote media manifests, progress callbacks, and artifact upload URLs, which are useful compatibility tools but should not define the public library API.

The binary exposes:

- `GET /health`
- `POST /render`

Run it with:

```bash
cargo run -p lumen-server --features cli --bin lumen-server -- --bind 127.0.0.1:8080
```

Set `--token` or `LUMEN_SERVER_TOKEN` to require `Authorization: Bearer ...` for render requests.

## Service Layer

The public service layer is built around four extension points:

- `RenderQueue`: enqueue, lease, acknowledge, retry, and heartbeat render jobs.
- `RenderExecutor`: execute a leased render job.
- `ArtifactStore`: persist render artifacts and return stable artifact refs.
- `ProgressSink`: publish render progress events.

Included building blocks:

- `InMemoryRenderQueue` for tests and single-process development.
- `LocalRenderExecutor` for executing renders in the current process.
- `NoopProgressSink` for callers that do not need progress events.
- `CallbackProgressSink` for generic HTTP progress callbacks.
- `PresignedUrlArtifactStore` for S3-compatible or R2-style pre-signed uploads.
- `RenderService::process_next` for leasing, executing, storing, and acking one queued job.

Downstream applications that need different job contracts should build their own API around these service traits instead of depending on the binary request/response types.

## Development

```bash
cargo check -p lumen-server --lib
cargo check -p lumen-server --features cli --bin lumen-server
cargo test -p lumen-server --lib
```
