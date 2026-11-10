---
"@lumiscia/lumen-sdk": patch
---

Adds `check`, `getMedia` and `deleteMedia` to the client, and retries multipart upload parts that fail with a network error, a server error or a rate limit.
