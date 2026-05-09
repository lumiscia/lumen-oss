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

The exposed Axum API is intentionally binary-owned. Its request shape supports
remote media manifests, progress callbacks, and artifact upload URLs, which are
useful compatibility tools but should not define the public library API.

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
- `CallbackProgressSink` for generic HTTP progress callbacks.
- `PresignedUrlArtifactStore` for S3-compatible or R2-style pre-signed uploads.
- `RenderService::process_next` for leasing, executing, storing, and acking one queued job.

Production users should provide their own durable implementations. For example,
a hosted platform can keep a private crate that implements a RunPod executor,
a Cloudflare Queue or SQS-backed `RenderQueue`, and any account/auth/billing
policy, while still depending on the public `lumen-server` service traits and
render types.

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

The standalone binary keeps the compatibility HTTP adapter private to the
executable. Downstream applications that need different job contracts should
build their own API around the service traits instead of depending on the
binary request/response types.

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
