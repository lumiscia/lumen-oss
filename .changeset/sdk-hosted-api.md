---
"@lumiscia/lumen-sdk": minor
---

The SDK now works against the hosted Lumiscia API out of the box. The default `baseUrl` is `https://api.lumiscia.com/v1`, the public API that accepts API keys. The old default, `https://lumiscia.com/api/v1`, is the dashboard proxy, which only accepts sessions. `RenderJob.costCents` is now `costMicros` (millionths of a US dollar), which is what the API returns, and the never-returned `outputUrl` and `outputExpiresAt` fields are gone. Render events from the hosted socket's `{ type, state }` messages and from `lumen-server`'s flat messages are now normalized into the same `RenderEvent`. `waitForRender` rejects with a `LumenRenderError` (an `Error` with `code`, `renderId`, and the server's message) when a render fails, instead of `undefined`.
