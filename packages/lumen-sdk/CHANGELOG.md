# @lumiscia/lumen-sdk

## 0.5.1

### Patch Changes

- Updated dependencies [6285ab0]
- Updated dependencies [87164d8]
  - @lumiscia/lumen-shared@0.5.1
  - @lumiscia/lumen-types@0.5.1

## 0.5.0

### Minor Changes

- 4df3509: Diagnostics reach the TypeScript packages. The wasm bindings add `check(json, fonts?)`, which returns what is wrong with a composition without loading it, and a composition `loadComposition` rejects throws with the same `diagnostics`. `LumenError` in the SDK has `diagnostics`, filled when a render is rejected for an invalid composition. The preview and both `Player` components take `onError`, which receives each failure with its diagnostics. `LumenDiagnostic` is exported from `@lumiscia/lumen-types`.
- d60eaad: `POST /renders` on `lumen-server` accepts `quality`, a quality preset name such as `"high"`, and `codec`, `"h264"`, `"hevc"` or `"av1"`. Both win over the composition's `render_settings`. `RenderOptions.quality` and `RenderOptions.codec` in the SDK send them, and `QualityPreset` types the preset name.

### Patch Changes

- 92b28e3: Every package now has a README, so its npm page describes it.
- Updated dependencies [6cddffa]
- Updated dependencies [db420ff]
- Updated dependencies [d21228a]
- Updated dependencies [4df3509]
- Updated dependencies [428e0a7]
- Updated dependencies [8a28aa1]
- Updated dependencies [08a87a7]
- Updated dependencies [3861bfd]
- Updated dependencies [53e1e41]
- Updated dependencies [3665a2c]
- Updated dependencies [b7342d2]
- Updated dependencies [f31d22c]
- Updated dependencies [8857666]
- Updated dependencies [48ad23c]
- Updated dependencies [c491e28]
- Updated dependencies [92b28e3]
- Updated dependencies [1d76f93]
- Updated dependencies [404f117]
- Updated dependencies [ef422d1]
- Updated dependencies [5db9958]
  - @lumiscia/lumen-types@0.5.0
  - @lumiscia/lumen-shared@0.5.0

## 0.4.0

### Minor Changes

- 9dff5b4: The SDK now works against the hosted Lumiscia API out of the box. The default `baseUrl` is `https://api.lumiscia.com/v1`, the public API that accepts API keys. The old default, `https://lumiscia.com/api/v1`, is the dashboard proxy, which only accepts sessions. `RenderJob.costCents` is now `costMicros` (millionths of a US dollar), which is what the API returns. Render events from the hosted socket's `{ type, state }` messages and from `lumen-server`'s flat messages are now normalized into the same `RenderEvent`. `waitForRender` rejects with a `LumenRenderError` (an `Error` with `code`, `renderId`, and the server's message) when a render fails, instead of `undefined`.
- f7e7b30: Every SDK method now throws one `LumenError` (`code`, `status?`, `requestId?`, `renderId?`, `retryable?`) instead of returning `{ error }` or throwing `LumenRenderError`, and `render`, `getRender` and `waitForRender` all return one `Render`. `render` and `getRender` used to return `{ id, render, error, cached }` with an `id` that could be undefined; now `const { id } = await lumen.render(composition)` needs no `!`, and `await lumen.render(composition)` throws on a rejected composition. `Render.url` is absolute (a relative output URL from lumen-server is resolved under `baseUrl`, so a server behind a path prefix such as `https://host/lumen` keeps its prefix), and `waitForRender` resolves with the finished `Render` instead of the completed event (the event still reaches `onEvent`).

  `lumen.download(renderOrId)` returns the `Response` for the output, sending the API key only when the output is on the API's own origin, and replaces `getRenderArtifact`, which always requested `/renders/:id/artifact` and returned a `Blob`, instead of following the render's own `url`. Network failures become `LumenError` with code `network_error`, a malformed success body becomes `invalid_response`, and a media or upload response without the fields the SDK needs becomes `invalid_response` instead of resolving `undefined`, and an aborted `signal` still rejects with the abort reason, including while a response body is being read. The error body read is lumen-server's and the hosted API's `{ code, error, requestId }`; the older `{ code, message }` body is no longer recognised.

  `RenderJob`, `RenderResult` and `LumenApiError` are replaced by `Render`, `RenderFailure` and `LumenError`, and every type the methods mention is now exported.

- 302f50c: `waitForRender` now treats `GET /renders/:id/progress` as the truth and the socket as a wake-up. It reads the render, holds one socket open for up to a minute to hear about progress, then reads again, with sockets at least 5 s apart (plus up to 25% jitter) and the gap doubling to 30 s while they keep failing, so a wait costs about two requests a minute and still works where sockets don't. A rate limit never ends a wait: it makes every wait on the same `Lumen` hold off for the API's `Retry-After`, or a growing delay without one, for reads and socket upgrades alike. Server errors and network failures are retried with backoff, and end the wait only after ten minutes without a successful read; a 401 or 404 throws at once, and `signal` cancels from any point in the wait. `onEvent` is replaced by `onProgress`, which receives `{ id, status, progress }` from sockets and from reads and never goes backwards. The `websocket` option, `subscribeToRender`, the `ws` dependency, the `./cloudflare` export and the auth frame are gone: the SDK opens the socket itself, with the global `WebSocket` and an `Authorization` header on Node 22 and with a `fetch` upgrade on Cloudflare Workers. Browsers cannot set that header, so there the wait falls back to reading every 5 to 30 s. `engines.node` is now `>=22`. A render that finished is understood even when the API has no progress record for it, and a failed render's code is read from `errorCode` (lumen-server) or `stage` (hosted).

  Six upload methods become one: `uploadMedia({ body, fileName?, contentType?, sizeBytes?, signal? })`. A `File` or typed Blob needs nothing else, and its size, name and type are taken from it; a buffer also needs a `contentType` or a `fileName` with a known extension, and a stream needs `sizeBytes` too. Streams and bodies over 64 MiB go up in 8 MiB parts and everything else in one request, so `multipart`, `partSizeBytes`, `createMediaUpload`, `createMediaMultipartUpload`, `uploadMediaMultipart`, `uploadMediaMultipartPart` and `completeMediaMultipartUpload` are removed. A file's content type comes from `contentType`, the blob's type, or its file name's extension (`mp4`, `mov`, `webm`, `mp3`, `wav`, `png`, `jpg` and similar); if none gives one, `uploadMedia` throws `invalid_upload` before any request, because the API accepts only audio, image and video types. A stream that is aborted, or produces more than `sizeBytes`, is cancelled and unlocked. `media` in `render` accepts an upload directly: `media: { hero: await lumen.uploadMedia({ body: file }) }`, so `mediaReference` is optional.

  The exported types `CreateMediaUploadOptions`, `CreateMediaUploadResult`, `CreateMultipartUploadResult`, `CompleteMultipartUploadOptions`, `MediaUploadPart`, `MultipartUploadOptions`, `LumenWebSocket`, `LumenWebSocketFactory`, `LumenWebSocketFactoryOptions`, `RenderEvent`, `RenderEventHandlers` and `RenderEventSubscription` are removed; `RenderProgress` is added. A multipart upload that fails leaves its uploaded parts on the server, because the API has no route to abort one.

  `@lumiscia/lumen-sdk` now re-exports everything in `@lumiscia/lumen-shared`, so `Composition`, the expression helpers and their types come from one import, and the package exports only `.`.

- f850ce9: Add `verifyWebhook({ body, headers, secret, toleranceSeconds? })`, exported from both the Node and Cloudflare entry points. It verifies a render webhook signed with the [Standard Webhooks](https://www.standardwebhooks.com) scheme (`webhook-id`, `webhook-timestamp`, and `webhook-signature` headers, signed with a `whsec_` secret) using Web Crypto, then returns the typed `RenderWebhookEvent`. If the webhook can't be verified or parsed, it throws a `LumenWebhookError` with a `code`: `missing_headers`, `invalid_secret`, `invalid_signature`, `timestamp_out_of_range`, or `invalid_payload`.

### Patch Changes

- 7f838da: Fix SDK requests in browsers and Workers, where `fetch` no longer fails with "Illegal invocation". Multipart uploads now send the upload ID in the path the hosted API expects, stream uploads work on Node, and the API key is only sent to the API's own origin, not to an `uploadUrl` on another host.
- Updated dependencies [c1c57b2]
- Updated dependencies [b6ece6b]
- Updated dependencies [6b3a821]
- Updated dependencies [0003560]
- Updated dependencies [75572c3]
- Updated dependencies [8eac30a]
- Updated dependencies [7c5ccae]
- Updated dependencies [fb4c25b]
- Updated dependencies [eb354e6]
- Updated dependencies [4f3894f]
- Updated dependencies [93fb454]
- Updated dependencies [5a0939c]
- Updated dependencies [37500c3]
- Updated dependencies [f35c132]
- Updated dependencies [0d47151]
- Updated dependencies [e3d02e6]
- Updated dependencies [ca87eaa]
  - @lumiscia/lumen-shared@0.4.0
  - @lumiscia/lumen-types@0.4.0

## 0.3.1

### Patch Changes

- @lumiscia/lumen-shared@0.3.1
  - @lumiscia/lumen-types@0.3.1

## 0.3.0

### Patch Changes

- 482ec57: Update runtime dependencies to their latest releases.
- Updated dependencies [2c68ade]
- Updated dependencies [38a05a8]
- Updated dependencies [5726783]
- Updated dependencies [577d694]
- Updated dependencies [d52b7d6]
- Updated dependencies [b314e00]
- Updated dependencies [3f45d73]
- Updated dependencies [076bf72]
- Updated dependencies [2b50b73]
- Updated dependencies [f054aaf]
- Updated dependencies [b0bbf52]
- Updated dependencies [97f55bf]
- Updated dependencies [1b30cb7]
- Updated dependencies [1ab946d]
- Updated dependencies [d658d6c]
- Updated dependencies [b88aeda]
- Updated dependencies [4e61e7a]
  - @lumiscia/lumen-types@0.3.0
  - @lumiscia/lumen-shared@0.3.0

## 0.2.2

### Patch Changes

- @lumiscia/lumen-shared@0.2.2
- @lumiscia/lumen-types@0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.
- cb6c9fa: Add composition audio track and clip builder helpers with canonical timeline output.

### Patch Changes

- Updated dependencies [072169a]
- Updated dependencies [cb6f4dc]
- Updated dependencies [cb6c9fa]
  - @lumiscia/lumen-shared@0.2.0
  - @lumiscia/lumen-types@0.2.0

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.

### Patch Changes

- Updated dependencies [906481a]
  - @lumiscia/lumen-shared@0.1.0
  - @lumiscia/lumen-types@0.1.0
