---
"@lumiscia/lumen-bindings": minor
---

`lumen-server` keeps one record per render, so polling, `GET /renders/:id/progress` and the socket always agree. `GET /renders/:id/artifact` answers `409` (`artifact_not_ready`, or the render's own error code) instead of `404` until the render succeeds, sockets close after the final event, `render.status` reports `processing` while a render runs, `render.outputUrl` carries the artifact URL, `createdAt` is RFC 3339, and `progress.updatedAt` and `progress.resolution` are gone. `--progress-min-delta` rejects values outside 0 to 1.
