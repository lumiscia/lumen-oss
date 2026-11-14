---
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-bindings": minor
---

`lumen-server`'s HTTP API has one definition: the new `lumen-server-api` crate holds its request, response, socket-event and error types, the server (de)serialises with them, and `definitions/openapi.json` is generated from them for other implementations of the API. `@lumiscia/lumen-types` exports the generated types (`RenderRequest`, `Render`, `RenderEvent`, `ErrorBody`, `ErrorCode`, `Media`, …), with `errorCodes` (each code and what it means) and `outputFiles` (each output format's content type and extension). `VideoCodec` and `VideoBitDepth` keep their names and now come from that schema.

Every object on the wire is camelCase. The composition inside a request keeps its own format.

| Before                                      | Now                                                |
| ------------------------------------------- | -------------------------------------------------- |
| `bit_depth` (render request)                | `bitDepth`                                         |
| `output.jpeg_quality` (render request)      | `output.jpegQuality`                               |
| `render.outputUrl`                          | `render.url`                                       |
| `progress.artifactUrl`                      | `render.url`                                       |
| `progress.renderId`                         | `render.id`                                        |
| `progress.state`                            | `render.status`                                    |
| `progress.progress`                         | `render.progress`                                  |
| `progress.error`                            | `render.error.message`                             |
| `progress.errorCode`                        | `render.error.code`                                |
| `progress.retryable`                        | `render.error.retryable`                           |
| `progress.durationMs`                       | `render.durationMs`                                |
| `progress.outputBytes`                      | `render.outputBytes`                               |
| `progress.stage`, `progress.organizationId` | removed (`render.status`, `render.organizationId`) |

A render has one shape. `POST /renders`, `GET /renders/:id`, `POST /renders/:id/cancel` and `GET /renders/:id/progress` all return it as `render`, and `/progress` no longer adds a second `progress` object:

```json
{
  "render": {
    "id": "r_…",
    "status": "succeeded",
    "progress": 1,
    "url": "/renders/r_…/artifact",
    "createdAt": "2026-11-14T00:00:00Z",
    "inputHash": "…",
    "organizationId": "self-hosted",
    "costMicros": 0,
    "outputBytes": 48213,
    "durationMs": 912
  }
}
```

When a render fails on a media source, its `error` (and the body of a `409` from its artifact) carries `media`, the engine's `MediaFailure` from `definitions/runtime.schema.json`:

```json
{
  "code": "media.decode",
  "message": "…",
  "retryable": false,
  "media": { "code": "media.decode", "source": "hero.png", "wanted": ["image"], "details": "…" }
}
```

A `codec` or `bitDepth` that isn't one of the API's values is now refused with `422` and `invalid_request_body` instead of `invalid_quality`. Socket events keep their shape.

The SDK sends and reads the new names; its public types are unchanged.
