---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-sdk": minor
---

**Breaking (HTTP, `lumen-server`):** renders can be cancelled, the queue is bounded, and a render can end as `cancelled`.

`POST /renders/:id/cancel` cancels a queued or running render. A queued render ends as `cancelled`, gives its queue place back and never starts (`200`); a running one answers `202` and stops at its next frame boundary (or at once while it waits for GPU device room, and during the audio and encoder finish after the last frame), closing its encoder, deleting its temporary output, releasing its media and freeing its slot. A render that has ended is returned unchanged with `200`, so the request is idempotent, and an unknown id is `404`. A cancelled render has `status: "cancelled"` and the code `render_cancelled` (not retryable) in `error`, `progress.errorCode` and `retryable`; `GET /renders/:id/artifact` answers `409` with that code, and its socket sends `render.cancelled` and closes.

At most `--max-queued-renders` renders (`LUMEN_SERVER_MAX_QUEUED_RENDERS`, default 32) wait for a slot. Past that, `POST /renders` answers `503` with `render_queue_full` at once and keeps nothing; `0` refuses every render that can't start immediately. Before, any number queued.

**Migrate:** clients that polled until `succeeded` or `failed` must also stop on `cancelled`. Clients that submit more than 32 renders ahead of the running ones must retry on `503` or raise `--max-queued-renders`. `Mp4Render::open` takes a `CancelToken` as a new last argument (`CancelToken::new()` for none), and `Mp4Render::set_cancel` is gone.

TypeScript SDK: `RenderStatus` gains `"cancelled"`, and `lumen.cancelRender(id)` calls the route. `waitForRender` and `download` throw a `LumenError` with the non-retryable code `render_cancelled` for a cancelled render, where before a `cancelled` status was rejected as `invalid_response`.

```ts
import { LumenError } from "@lumiscia/lumen-sdk";

const render = await lumen.render(composition);
await lumen.cancelRender(render.id);
await lumen.waitForRender(render).catch((error) => {
  if (!(error instanceof LumenError && error.code === "render_cancelled")) throw error;
});
```

```sh
curl -X POST -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/renders/r_123/cancel
```

The `lumen_server::render` library gets the same hook: `RenderOptions::cancel` takes a `CancelToken`, and `Mp4Render::open` takes one for a render opened by hand. A render checks it while waiting for GPU device room, before each frame and while finishing, and ends with the new `ErrorCode::Cancelled` (`render_cancelled`). `RenderOptions` has a new field, so code that builds it without `..RenderOptions::default()` must add `cancel`.

```rust
let cancel = CancelToken::new();
let options = RenderOptions { cancel: cancel.clone(), ..RenderOptions::default() };
// elsewhere: cancel.cancel();
```

The CPU upload encode path reads frames back through a ring of three buffers while the next frame renders, instead of waiting for each readback. Output is byte-identical; memory is bounded by the ring.
