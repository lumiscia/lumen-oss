---
"@lumiscia/lumen-bindings": minor
---

`lumen-server` errors now use the shape the SDK reads, `{ "code", "error" }`, so `client.render()` and `getRender()` report `render_not_found` and `invalid_project_payload` rather than `http_404` and a status line. Malformed bodies and bodies over the 2 MiB limit answer with this shape too, with the codes `invalid_request_body` and `request_too_large`. A failed render keeps its real code and `retryable`: `GET /renders/:id` and `/progress` return them as `render.error`, `progress` gains `errorCode` and `retryable`, and the `render.failed` socket event no longer always says `render_failed`.
