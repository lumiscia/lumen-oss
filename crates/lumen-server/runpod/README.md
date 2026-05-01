# Lumen Runpod Worker

Build this image from the `lumiscia` directory, one level above this repository,
so the Dockerfile can copy both `lumen` and the sibling `rust-skia` path
dependency:

```sh
cd ..
container build -f lumen/crates/lumen-server/runpod/Dockerfile -t lumen-runpod:latest .
```

Push the image to a registry Runpod can pull from, then create a queue-based
Serverless endpoint from that image.

Configure `apps/api` with:

- `RUNPOD_API_KEY`
- `RUNPOD_ENDPOINT_ID`
- `RUNPOD_CALLBACK_SECRET`
- `PUBLIC_API_BASE_URL`
- optional `RUNPOD_EXECUTION_TIMEOUT_MS`
- optional `RUNPOD_TTL_MS`

The API submits `/run` jobs directly to Runpod. The worker posts live progress
to `/v1/runpod/progress/:renderId` and uploads the final MP4 to
`/v1/runpod/artifacts/:renderId`; both callback URLs are signed per render.

The image builds `lumen-runpod` with `--features vulkan` for Linux GPU rendering.
For local macOS GPU checks, build the server with `--features metal` instead.
