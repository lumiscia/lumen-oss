# @lumiscia/lumen-sdk

## 0.6.0-beta.1

### Patch Changes

- Updated dependencies [ce70b5c]
- Updated dependencies [cd82e6d]
  - @lumiscia/lumen-types@0.6.0-beta.1
  - @lumiscia/lumen-shared@0.6.0-beta.1

## 0.6.0-beta.0

### Minor Changes

- 4bacce6: **Video output with an alpha channel.** Two new output formats hold alpha: `mov`, ProRes 4444 from `prores_ks` (`yuva444p10le`) with AAC audio, and `webm`, VP9 from `libvpx-vp9` (`yuva420p`, tagged `alpha_mode=1`) with Opus audio from `libopus`. A render asks for alpha with the request's `alpha`, `lumen --alpha` or the SDK's `render(..., { alpha: true })`, which win over `render_settings.alpha`, as `codec` does. The default stays opaque, and an `mp4` is unchanged.

  ```json
  { "render_settings": { "width": 1920, "height": 1080, "alpha": true } }
  ```

  ```json
  { "composition": { "...": "..." }, "alpha": true, "output": { "format": "webm" } }
  ```

  The colour is straight, as a PNG of the frame holds it: the premultiplied linear colour is unpremultiplied in linear light, sRGB-encoded and converted to BT.709 limited range once, from float, with the same ordered dither as opaque output. Alpha is full range and never dithered, and a fully transparent pixel is black with neutral chroma. VP9's 4:2:0 chroma is the alpha-weighted average of each 2x2 block, `Σ aᵢ·cᵢ / Σ aᵢ`, so transparent pixels don't darken edges.

  `codec` takes `prores` and `vp9` alongside `h264`, `hevc` and `av1`. A `mov` or `webm` without a codec gets its own; `mp4` holds `h264`, `hevc` and `av1`, `mov` holds `prores` and `webm` holds `vp9`, and anything else is refused with `invalid_output`. ProRes is always 10-bit, VP9 with alpha 8-bit, and either also encodes opaque video. Every built-in preset but `lossless` has `prores_ks` settings (quantiser 8, 4 and 2), and every one has `libvpx-vp9` settings (constant quality, `cpu-used` 0 to 5 as its speed). No hardware encoder writes alpha: auto selection goes straight to the software encoder, and a pinned hardware encoder is refused with `invalid_quality`. Alpha in an `mp4`, or with `h264`, `hevc` or `av1`, is an error, never an opaque video, as is a `prores_ks` option that leaves a profile other than 4444 or, with alpha, an `alpha_bits` other than 16, either of which drops the alpha plane. A composition with audio is refused with `invalid_output` when the render is requested if this FFmpeg has no encoder for the container's audio (`libopus` for WebM, AAC otherwise).

  **Breaking, Rust API:** `lumen_server::render::resolve_encoder` takes the `OutputFormat` after the composition and returns a `ServerRenderError` (`invalid_output` or `invalid_quality`) rather than an `anyhow::Error`. `OutputFormat` has `Mov` and `Webm`, `OutputCodec` and `EncodeCodec` have `ProRes` and `Vp9` (so `EncodeCodec::ALL` has five), and `OutputSettings` has `alpha`. In `lumen-ffmpeg`, `VideoEncoderConfig::alpha`, `VideoEncoderChoice::with_alpha`, `QualityRequest::alpha`, `check_alpha` and `check_audio_encoder` are new, `EncoderFamily` has `Vpx` and `ProRes`, `PixelFormat` has `Yuva420p`, `Yuv444p10` and `Yuva444p10`, and `check_bit_depth` refuses 8-bit ProRes. `lumen_engine::gpu::YuvConverter` also converts to `Yuva420p`, `Yuv444p10` and `Yuva444p10`. Code matching these enums exhaustively needs the new arms.

  An audio encoder now writes the codec it was opened with, where it was always tagged AAC.

- dd03a5c: `expr` can be used as a template tag. ``expr<Vec2>`vec2(0, ${lift})` `` lowers each interpolated value the way builtin arguments are lowered, so built expressions, numbers, strings and tuples go in without stripping a leading `=` by hand. Raw expressions are bracketed where they meet an operator.
- d94ba0e: Expressions gain typed conditions. `Expr` has `gt`, `gte`, `lt`, `lte`, `eq` and `neq`, which evaluate to `Expr<boolean>`, and `and`, `or` and `not` on booleans. `when(condition, then, otherwise)` builds `if(..)` for numbers, vec2 and vec3 values, colours and strings, and `node(id, 'property')` reads another node's param.
- 16aae16: `model_3d` draws more of glTF:

  - **Vertex colours** (`COLOR_0`, RGB or RGBA, float or normalised 8- or 16-bit) multiply the base colour in linear light, as its factor and texture do, lit or unlit, and their alpha blends or masks with it.
  - **A second set of texture coordinates** (`TEXCOORD_1`): each map reads the set its `texCoord` names. A map naming a third set is left out.
  - **`KHR_texture_transform`** on every map: offset, rotation and scale, and its own `texCoord`, which overrides the map's. Rotation turns as the Khronos sample renderer and three.js do.
  - **Morph targets**: position and normal displacements, blended on the GPU by the mesh's or node's default weights and by clips animating `weights`, before skinning. Tangent displacements are ignored. A mesh may have up to 256 targets, and a model up to 4,000,000 displacements (one for each vertex of each target) and 1,048,576 weights; a scene draws up to 1,048,576 weights in a frame. A weight may be at most 10,000 either way (a clip sample overshooting it is held to it), and a vertex its targets would move past what a float holds at that weight is rejected. Shadows hold a morphed mesh at whatever weights it is drawn with. Morphed normals are unit length before skinning, falling back to the unmorphed normal where targets cancel it, and every vertex normal is normalised before it is interpolated. Draco-compressed primitives with morph targets are rejected. A model with still weights is kept across frames like any still model, and only the weights are uploaded each frame while a clip plays them. A clip's `duration` counts its weight channels.
  - **The model's own lights and cameras**, opt-in. `lights: true` adds its `KHR_lights_punctual` lights where its nodes place them. Their units are `light_3d`'s: a directional light's lux and a point or spot light's candela are used as they are, so a point light of intensity `I` lights a surface `d` metres away as a directional light of `I / d²` does, and a spot light fades from its inner cone to its outer one. A light whose colour times intensity overflows is rejected. They cast no shadows. `camera` views the scene from one of the model's cameras, by name or else index, placed by the first node that places it without that node's scale; it counts as a `camera_3d` at the model, so the first camera in the scene still wins, and the output's aspect ratio is used. `modelInfo` and an uploaded model's `metadata` list the cameras (`index`, and `name` when named).

  Points, lines, and triangle strips and fans are still not drawn, and now raise a `scene.model_primitives_skipped` warning naming the model and how many were left out.

  Nothing in a composition breaks, but models now look as their files describe: vertex colours tint them, maps naming `TEXCOORD_1` read it instead of the first set, and morph targets move them. Files requiring `KHR_texture_transform` or `KHR_lights_punctual` are no longer rejected. `LumenModelInfo` gains `cameras`, and the Rust `ModelInfo` a `cameras` field.

  ```json
  {
    "id": 1,
    "type": "model_3d",
    "params": { "source": "asset:robot", "animation": "Talk", "lights": true, "camera": "Closeup" }
  }
  ```

  ```ts
  import { createLumenBindings } from "@lumiscia/lumen-bindings";
  import { nodes } from "@lumiscia/lumen-shared";

  const robot = nodes.model_3d({
    source: "asset:robot",
    animation: "Talk",
    lights: true,
    camera: "Closeup",
  });
  const { modelInfo } = await createLumenBindings().preview();
  const { cameras } = modelInfo(bytes); // [{ index: 0, name: "Closeup" }]
  ```

- a37b057: A model's animation clips and materials can be listed, so `animation` and `texture_material` no longer have to be guessed from an error.

  An uploaded model's media object has `kind: "model"` and a `metadata` listing them, read once when the upload completes and returned by `GET /media/:id`:

  ```json
  {
    "kind": "model",
    "metadata": {
      "animations": [
        { "index": 0, "name": "Walk", "duration": 0.7 },
        { "index": 1, "duration": 2 }
      ],
      "materials": [{ "index": 0, "name": "screen", "hasBaseColorTexture": false }],
      "cameras": []
    }
  }
  ```

  In the browser, `modelInfo` on the bindings reads a `.glb` and returns the same lists, throwing on a file a render could not read:

  ```ts
  const { modelInfo } = await createLumenBindings().preview();
  const { animations, materials } = modelInfo(new Uint8Array(await file.arrayBuffer()));
  ```

  `TemporaryMedia` has `kind` and `metadata` (`MediaKind`, `MediaMetadata`), `@lumiscia/lumen-types` has `LumenModelInfo`, and `uploadMedia` knows `.glb` file names. The error for an `animation` or `texture_material` the model lacks now lists at most 20 names, then how many more there are.

- fe146b5: **Breaking:** `lumenSchemaVersion` is now the Lumen `major.minor` a composition was written for, `"0.6"` for the 0.6 line, instead of the frozen `"0.1.0"`. An engine reads only its own line: it accepts its `major.minor` or a full `major.minor.patch` of that line (`"0.6.3"`), and rejects everything else, including today's `"0.1.0"`, with `document.schema_version`:

  ```
  this composition declares schema version 0.1.0; this engine reads 0.6, so it was written for a different Lumen version
  ```

  Every stored composition that declares `"lumenSchemaVersion": "0.1.0"` must change it to `"0.6"`. A composition that leaves the field out is still read as the engine's own version. 0.x minor releases are breaking, so there is no migration between lines.

  `Composition.toJSON()` in the SDK always writes the current version, so compositions built with the SDK need no change, and the `lumenSchemaVersion` option of `Composition` is removed. `@lumiscia/lumen-types` exports `lumenSchemaVersion` (`"0.6"`), `lumen_engine::SCHEMA_VERSION` is the Rust side, and the JSON schema constrains the property to `^0\.6(\.[0-9]+)?$`. `just set-schema-version <major.minor>` moves the engine constant, the tracked fixtures and docs, and the generated output together.

- ac4bc51: Compositions can be built as nested node values. `nodes` has one function per node kind, named after its `type`, that takes the node's params and input ports by name in one object and returns the `NodeInput` `addNode` takes, without adding anything. A port takes a node value as well as a node, and `composition.addOutput(source)` adds a `media_output` reading it. A value is added when a node reading it is, after its own inputs, so it gets the ids and JSON the same `addNode` calls would; the same object read twice is one node with two readers, and a value mutated into a cycle is an error. Ports keep their kinds: a scene value in a raster port, or a raster in `render_3d`'s `scene`, doesn't compile.

  ```ts
  import { Composition, nodes as n } from "@lumiscia/lumen-sdk";

  const scene = n.merge_3d({
    objects: [n.camera_3d({ position: [0, 1, 6] }), n.shape_3d({ shape: "torus" })],
  });
  const composition = new Composition().addOutput(
    n.raster_multimerge({
      layers: [
        n.render_3d({ scene, tone_mapping: "agx" }),
        n.glow({ glow_only: true, radius: 46, source: n.render_3d({ scene, exposure: -1 }) }),
      ],
    }),
  );
  ```

  `defineComposition`'s `build` may return a node value, and a `clip()` nests in one like any other node. `addNode` takes node values in its `inputs` too, and values may read nodes `addNode` returned.

  `@lumiscia/lumen-types` adds `nodeInputPorts`, each kind's input ports with their kind and whether they take a list, and `NodeParamPortNameClash`, which is `never`: the generated types stop compiling if a kind ever gives a param and an input port the same name.

  `addNode` itself still adds a node every time it is called. A failed `addNode` leaves the composition as it was, nested values included.

- 79817d9: **Breaking (HTTP, `lumen-server`):** renders can be cancelled, the queue is bounded, and a render can end as `cancelled`.

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

- f045a94: **Opt-in 10-bit video output.** A render can ask for 10 bits per sample with `hevc` or `av1`: the request's `bit_depth`, `lumen --bit-depth 10` or the SDK's `render(..., { bitDepth: 10 })`, which win over `render_settings.bit_depth`, which wins over a custom preset's `bit_depth` in the quality config, as `codec` does. The default stays 8 everywhere.

  ```json
  { "render_settings": { "width": 1920, "height": 1080, "codec": "hevc", "bit_depth": 10 } }
  ```

  The renderer quantises its float working space straight to 10-bit BT.709 limited range (Y 64 to 940, C 64 to 960), with the same ordered dither as 8-bit. `libx265` and `libsvtav1` encode `yuv420p10le` (HEVC Main 10, AV1 Main); `hevc_vulkan`, `av1_vulkan` and NVENC take P010 where the device supports it. H.264 is always 8-bit and VideoToolbox has no 10-bit path, so asking either for 10 bits is an error. An encoder that can't write 10 bits is skipped like one that won't open, and a pinned one fails the render: a 10-bit request never comes out at 8.

  **CPU encoders now get dithered YUV from the GPU.** Every encoder that takes CPU frames, at either depth, receives YUV 4:2:0 converted on the GPU from float colour and quantised once, so 8-bit CPU output is dithered in its final domain too and no longer passes through swscale. Its chroma is the 2x2 average of encoded colour, as the Vulkan and NVENC paths already were, so 8-bit `libx264`, `libx265` and `libsvtav1` output changes slightly.

  **Rust API:** `VulkanVideoTarget::nv12(renderer, size)` is `VulkanVideoTarget::new(renderer, size, BitDepth, dither)`, and `CudaNvencTargetPool::rgba8(renderer, size)` is `CudaNvencTargetPool::new(renderer, size, BitDepth, dither)`. `VulkanVideoFrame::new` takes a `VulkanFrameLayout` (`nv12` or `p010`) in place of a width and height. `lumen_engine::gpu::YuvConverter` converts a frame to `Nv12`, `P010`, `Yuv420p` or `Yuv420p10`. In `lumen-ffmpeg`, `VideoEncoderConfig::bit_depth`, `VideoEncoderChoice::with_bit_depth`, `QualityRequest::bit_depth`, `check_bit_depth` and `MuxedEncoder::cpu_input_format` are new, `PixelFormat` has `Yuv420p` and `Yuv420p10`, and `write_video_frame` takes YUV frames in the encoder's own format as well as RGBA8, which a 10-bit encoder refuses.

  Output streams are now tagged with centred chroma (`chroma_location=center`), which is where the 2x2 average puts it. An encoder option that would change the pixel format, and with it the bit depth, is an error when the encoder opens.

### Patch Changes

- d09b553: Documentation for 3D. The `lumen-sdk`, `lumen-shared` and `lumen-preview` READMEs link to the new 3D guide, and `lumen-shared`'s shows a scene. The descriptions of a 3D material's `color` and `shadow_catcher` params, which `lumen-types` carries as doc comments and `lumen-bindings` as node metadata, now say that only an image plane has a raster to multiply by `color` or to count in `shadow_catcher`. Nothing about how a composition renders changes.
- Updated dependencies [ec34c26]
- Updated dependencies [d09b553]
- Updated dependencies [4bacce6]
- Updated dependencies [8ef4ee5]
- Updated dependencies [97c7016]
- Updated dependencies [c4d3140]
- Updated dependencies [c657075]
- Updated dependencies [dd03a5c]
- Updated dependencies [d94ba0e]
- Updated dependencies [c4d3f90]
- Updated dependencies [dc06fed]
- Updated dependencies [d3e4ce9]
- Updated dependencies [89568da]
- Updated dependencies [16aae16]
- Updated dependencies [b9cc4f1]
- Updated dependencies [a37b057]
- Updated dependencies [1879afe]
- Updated dependencies [c0f96e6]
- Updated dependencies [5233133]
- Updated dependencies [fe146b5]
- Updated dependencies [ac4bc51]
- Updated dependencies [0382574]
- Updated dependencies [f045a94]
- Updated dependencies [ef94bcb]
- Updated dependencies [49b2878]
- Updated dependencies [3ab8aa0]
  - @lumiscia/lumen-types@0.6.0-beta.0
  - @lumiscia/lumen-shared@0.6.0-beta.0

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
