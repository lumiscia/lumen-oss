# Lumen Server

`lumen-server` provides provider-neutral rendering service primitives for Lumen.
It is intended to be used as a dependency by applications that want to build a
self-hosted or hosted render platform around the Lumen renderer.

The crate intentionally does not include assumptions about any infrastructure
provider. RunPod, Vast, Cloudflare, object storage, durable queues, hosted auth,
billing, and deployment automation should live in downstream crates or
applications.

## Targets

`lumen-server` ships both:

- a library target, `lumen_server`, for embedding in custom render platforms
- a generic binary target, `lumen-server`, for local HTTP rendering

The binary exposes:

- `GET /health`
- `POST /render`

By default it binds to `127.0.0.1:8080`. Override that with
`--bind` or `LUMEN_SERVER_BIND`. Set `--token` or `LUMEN_SERVER_TOKEN` to
require `Authorization: Bearer ...` for render requests.

```sh
cargo run -p lumen-server --features cli --bin lumen-server -- --bind 127.0.0.1:8080
```

## Architecture

The public service layer is built around four extension points:

- `RenderQueue`: enqueue, lease, acknowledge, retry, and heartbeat render jobs.
- `RenderExecutor`: execute a leased render job.
- `ArtifactStore`: persist render artifacts and return stable artifact refs.
- `ProgressSink`: publish render progress events.

The crate includes small local building blocks:

- `InMemoryRenderQueue` for tests and single-process development.
- `LocalRenderExecutor` for executing renders in the current process.
- `NoopProgressSink` for callers that do not need progress events.

Production users should provide their own implementations. For example, a hosted
platform can keep a private crate that implements a RunPod executor, a Cloudflare
or S3-compatible artifact store, and a durable queue, while still depending on
the public `lumen-server` service traits and render types.

## Example Shape

```rust
use lumen_server::service::{
    InMemoryRenderQueue, LocalRenderExecutor, NoopProgressSink, RenderService,
};

let service = RenderService::new(
    InMemoryRenderQueue::new(),
    LocalRenderExecutor,
    my_artifact_store,
    NoopProgressSink,
);
```

## What Belongs Outside This Crate

Provider adapters and hosted-platform concerns should be implemented outside
`lumen-server`, including:

- RunPod or Vast worker loops
- Cloudflare Queues, Durable Objects, R2, or other cloud-specific bindings
- billing, credits, accounts, teams, and dashboards
- customer webhooks and hosted project storage
- deployment scripts and provider-specific Docker images

This keeps the open crate reusable without forcing the hosted Lumen platform's
infrastructure choices onto self-hosted users.
