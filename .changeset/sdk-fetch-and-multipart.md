---
"@lumiscia/lumen-sdk": patch
---

Fix SDK requests in browsers and Workers, where `fetch` no longer fails with "Illegal invocation". Multipart uploads now send the upload ID in the path the hosted API expects, stream uploads work on Node, and the API key is only sent to the API's own origin, not to an `uploadUrl` on another host.
