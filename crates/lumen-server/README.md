# lumen-server

`lumen-server` provides provider-neutral rendering service primitives for Lumen plus a small local HTTP binary.

For production usage, the smoothest path is the hosted platform at [lumiscia.com](https://lumiscia.com). Self-hosting this crate is possible, but you will still need GPU machines, queueing, artifact storage, auth, deployment, monitoring, and provider-specific worker logic.

This crate is experimental and its service traits may change.

## Platform Notes

Native rendering requires a GPU.

Supported targets today are:

- Linux with Vulkan rendering and optional CUDA/NVENC interop when built with `vulkan,cuda`.
- macOS with Metal rendering and VideoToolbox-oriented media paths when built with `metal`.

## Targets

`lumen-server` ships both:

- a library target, `lumen_server`, for embedding in custom render platforms
- a binary target, `lumen-server`, for local HTTP rendering

The exposed Axum API is intentionally binary-owned. The HTTP binary accepts the same public render payload shape as the hosted Lumen API:

```json
{
  "composition": {
    "timeline": { "fps": 30, "duration_frames": 120 },
    "render_settings": { "width": 1280, "height": 720 },
    "nodes": [],
    "connections": []
  },
  "media": {
    "hero": "https://example.com/hero.png",
    "music": "https://example.com/music.wav"
  }
}
```

Inside the composition, media nodes and audio clips should reference the manifest aliases (`"hero"`, `"music"`), not the URLs directly. The hosted API uses the same alias pattern, but its manifest values are uploaded media references like `lumen:<media_id>`. The self-hosted server does not implement media upload APIs, artifact staging, provider progress callbacks, billing, accounts, durable queueing, or durable artifact storage; it downloads direct `http`/`https` media URLs, renders in a local background task, stores the MP4 artifact in memory, and exposes it at `GET /renders/:id/artifact`.

S3-compatible artifact storage is planned for a future self-hosted release. For now, the HTTP binary's in-memory artifact storage is intended for local development and single-process testing, not durable production delivery.

`GET /renders/:id/progress` and `GET /renders/:id/socket` mirror the hosted progress shape. The WebSocket sends SDK render events like:

```json
{
  "type": "render.progress",
  "renderId": "r_123",
  "progress": 0.42
}
```

The binary exposes:

- `GET /health`
- `POST /renders`
- `GET /renders/:id`
- `GET /renders/:id/progress`
- `GET /renders/:id/socket`
- `GET /renders/:id/artifact`

Run it with:

```bash
cargo run -p lumen-server --features cli --bin lumen-server -- --bind 127.0.0.1:8080
```

When using the TypeScript SDK against the local server, set the SDK base URL to `http://127.0.0.1:8080`.

Set `--token` or `LUMEN_SERVER_TOKEN` to require `Authorization: Bearer ...` for render requests.

Progress WebSocket updates are coalesced by default. Set
`--progress-min-delta` or `LUMEN_SERVER_PROGRESS_MIN_DELTA` to control the
minimum progress increase required before another non-terminal progress event is
broadcast. The value is a `0.0` to `1.0` fraction; the default is `0.02`.
Terminal updates, state changes, and stage changes are still broadcast
immediately. Set it to `0` to emit every renderer progress update.

Verbose render diagnostics are off by default. Set `--verbose-debug` or
`LUMEN_SERVER_VERBOSE_DEBUG=true` to enable detailed per-frame render progress
logs, CUDA/Vulkan device diagnostics, and frame timing output.

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
