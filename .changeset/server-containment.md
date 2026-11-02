---
"@lumiscia/lumen-bindings": minor
---

`lumen-server` is now safe to expose behind a token. Compositions can only read media from their manifest, downloads refuse non-public addresses on every connection (allow internal hosts with `--allow-media-host`), the token is checked on every route but `/health` before the body is read, and binding a non-loopback address without a token is refused at startup.

Other breaking changes: `job_id` and the `project` alias are gone, `webhookUrl` returns `400 webhooks_unsupported`, invalid compositions return `422` before anything is downloaded, renders run one at a time by default (`--max-concurrent-renders`), only the latest 16 finished renders are kept (`--retained-renders`), and manifests are capped at 256 entries and 2 GiB per job.
