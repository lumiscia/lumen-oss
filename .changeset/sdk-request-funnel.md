---
"@lumiscia/lumen-sdk": minor
---

Every SDK method now throws one `LumenError` (`code`, `status?`, `requestId?`, `renderId?`, `retryable?`) instead of returning `{ error }` or throwing `LumenRenderError`, and `render`, `getRender` and `waitForRender` all return one `Render`. `render` and `getRender` used to return `{ id, render, error, cached }` with an `id` that could be undefined; now `const { id } = await lumen.render(composition)` needs no `!`, and `await lumen.render(composition)` throws on a rejected composition. `Render.url` is absolute (a relative output URL from lumen-server is resolved under `baseUrl`, so a server behind a path prefix such as `https://host/lumen` keeps its prefix), and `waitForRender` resolves with the finished `Render` instead of the completed event (the event still reaches `onEvent`).

`lumen.download(renderOrId)` returns the `Response` for the output, sending the API key only when the output is on the API's own origin, and replaces `getRenderArtifact`, which always requested `/renders/:id/artifact` and returned a `Blob`, instead of following the render's own `url`. Network failures become `LumenError` with code `network_error`, a malformed success body becomes `invalid_response`, and a media or upload response without the fields the SDK needs becomes `invalid_response` instead of resolving `undefined`, and an aborted `signal` still rejects with the abort reason, including while a response body is being read. The error body read is lumen-server's and the hosted API's `{ code, error, requestId }`; the older `{ code, message }` body is no longer recognised.

`RenderJob`, `RenderResult` and `LumenApiError` are replaced by `Render`, `RenderFailure` and `LumenError`, and every type the methods mention is now exported.
