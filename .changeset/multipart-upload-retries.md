---
"@lumiscia/lumen-sdk": patch
"@lumiscia/lumen-bindings": patch
---

An upload in parts survives a dropped connection. `uploadMedia` sends a part up to three times when the connection fails, the response drops while it is read, the server answers with an error status of 500 or more, or the API rate limits it. It waits the `Retry-After` the response gives, or else 0.5 s and then 1 s, each lengthened by up to 25%. Another status, and a response that is not JSON, still fails the upload at once.

`lumen-server` accepts a part again at full size, so a retry of the last part no longer fails with `media_too_large`. A part's etag is now a hash of its bytes, so the same bytes keep the etag a lost response never delivered. One request at a time sends a part number: another gets `409` with the new code `upload_part_in_progress`, which `uploadMedia` also retries.
