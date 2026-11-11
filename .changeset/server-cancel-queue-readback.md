---
"@lumiscia/lumen-bindings": minor
---

**Breaking (HTTP, `lumen-server`):** renders can be cancelled, the queue is bounded, and a render can end as `cancelled`.

`POST /renders/:id/cancel` cancels a queued or running render. A queued render ends as `cancelled` and never starts (`200`); a running one answers `202` and stops at its next frame boundary, closing its encoder, deleting its temporary output, releasing its media and freeing its slot. A render that has ended is returned unchanged with `200`, so the request is idempotent, and an unknown id is `404`. A cancelled render has `status: "cancelled"` and the code `render_cancelled` (not retryable) in `error`, `progress.errorCode` and `retryable`; `GET /renders/:id/artifact` answers `409` with that code, and its socket sends `render.cancelled` and closes.

At most `--max-queued-renders` renders (`LUMEN_SERVER_MAX_QUEUED_RENDERS`, default 32) wait for a slot. Past that, `POST /renders` answers `503` with `render_queue_full` at once and keeps nothing; `0` refuses every render that can't start immediately. Before, any number queued.

**Migrate:** clients that polled until `succeeded` or `failed` must also stop on `cancelled`. Clients that submit more than 32 renders ahead of the running ones must retry on `503` or raise `--max-queued-renders`. The TypeScript SDK does not know `cancelled` yet.

```sh
curl -X POST -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/renders/r_123/cancel
```

The `lumen_server::render` library gets the same hook: `RenderOptions::cancel` takes a `CancelToken`, and `Mp4Render::set_cancel` does for a render opened by hand. A render checks it before each frame and ends with the new `ErrorCode::Cancelled` (`render_cancelled`). `RenderOptions` has a new field, so code that builds it without `..RenderOptions::default()` must add `cancel`.

```rust
let cancel = CancelToken::new();
let options = RenderOptions { cancel: cancel.clone(), ..RenderOptions::default() };
// elsewhere: cancel.cancel();
```

The CPU upload encode path reads frames back through a ring of three buffers while the next frame renders, instead of waiting for each readback. Output is byte-identical; memory is bounded by the ring.
