---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-sdk": patch
---

Adds `check`, `getMedia` and `deleteMedia` to the client, and retries multipart upload parts that fail with a network error, a dropped response body, a server error or a rate limit. The server accepts a multipart part sent again at full size, so a retried final part no longer fails with `media_too_large`.
