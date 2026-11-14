---
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-bindings": minor
---

The HTTP API is described by an OpenAPI 3.1 document, `definitions/openapi.json`, which replaces `definitions/api.schema.json`. It has what the schema had (the request, response, event and error types, now under `components/schemas`) and what it lacked: every route with its method, path and parameters, the body each takes, the status and body of each answer, and the error codes each status carries (`x-lumen-codes`). `@lumiscia/lumen-types` ships it with the two schemas it refers to:

```ts
import openapi from "@lumiscia/lumen-types/openapi.json" with { type: "json" };
import { apiOperations, apiVersion, type ApiOperationTypes } from "@lumiscia/lumen-types";

apiOperations.getRender; // { method: "GET", path: "/renders/{id}", request: "none", response: "json" }
type Created = ApiOperationTypes["createRender"]["response"]; // CreateRenderResponse
```

`lumen-server` builds its router from the same table of routes, so it serves the document's routes and no others, and `GET /health` answers `version`, the `info.version` of the document the server implements (`"0.6"`). The descriptions in `errorCodes` no longer start with a status: the document says which statuses carry which code.

A host of the API can be checked against the document with the SDK's suite, `LUMEN_API_URL=... LUMEN_API_TOKEN=... pnpm test:conformance` in this repository, with a profile file for what it does differently on purpose (`definitions/README.md`).

**Render webhooks** follow the Standard Webhooks payload as well as its signature. `verifyWebhook` returns the event with the render inside it, the same `Render` that `getRender` returns:

```ts
// Before
const event = await verifyWebhook({ body, headers, secret });
event.event; // "render.succeeded"
event.id;
event.artifactUrl;
event.error; // string

// Now
const event = await verifyWebhook({ body, headers, secret });
event.type; // "render.succeeded" | "render.failed"
event.timestamp;
event.data.id;
if (event.type === "render.succeeded") event.data.url;
if (event.type === "render.failed") event.data.error.message;
```

A host that sends webhooks posts `{ "type", "timestamp", "data": <render> }`; the flat body with `event`, `artifactUrl` and a string `error` is no longer accepted. `lumen-server` still delivers none.
