# Lumen Server

`lumen-server` is the shared GPU render server crate. Provider-specific binaries
adapt queueing and HTTP surfaces onto the same render executor:

- `lumen-runpod` polls RunPod Serverless jobs and posts RunPod results.
- `lumen-vast` runs a small HTTP server for Vast instances. It listens on
  `0.0.0.0:8080`, exposes `GET /health`, and accepts `POST /render` with a
  `RenderJobInput` JSON body.

Both Linux GPU images build with `--features vulkan,cuda` on Debian trixie and
run on `debian:trixie-slim`. The runtime installs the Debian Vulkan loader,
`vulkan-tools`, and FFmpeg 7 libraries, removes any baked NVIDIA ICD manifests,
sets `NVIDIA_DRIVER_CAPABILITIES=all` and `NVIDIA_VISIBLE_DEVICES=all`, and lets
the provider runtime inject and auto-discover the driver-matched NVIDIA Vulkan
ICD.

## RunPod

Build from the repository root:

```sh
container build -f crates/lumen-server/Dockerfile.runpod -t lumen-runpod:latest .
```

The `.github/workflows/deploy-runpod-serverless.yml` workflow publishes the
RunPod image to GHCR and runs `bun tooling/deploy-runpod-serverless.ts`.

Required GitHub secrets:

- `RUNPOD_API_KEY`

Optional GitHub variables:

- `RUNPOD_ENDPOINT_ID` updates an existing endpoint when set
- `RUNPOD_SERVERLESS_TEMPLATE_ID` updates an existing template when set
- `RUNPOD_SERVERLESS_ENDPOINT_NAME` defaults to `lumen-render`
- `RUNPOD_SERVERLESS_TEMPLATE_NAME` defaults to `lumen-render-template`
- `RUNPOD_SERVERLESS_MIN_WORKERS` defaults to `0`
- `RUNPOD_SERVERLESS_MAX_WORKERS` defaults to `2`
- `RUNPOD_SERVERLESS_IDLE_TIMEOUT` defaults to `5`
- `RUNPOD_SERVERLESS_EXECUTION_TIMEOUT_MS` defaults to `1800000`
- `RUNPOD_SERVERLESS_MIN_CUDA_VERSION` defaults to `12.8`
- `RUNPOD_SERVERLESS_GPU_TYPES` defaults to `NVIDIA RTX 2000 Ada Generation,NVIDIA RTX 4000 Ada Generation,NVIDIA L4`
- `RUNPOD_SERVERLESS_CONTAINER_DISK_GB` defaults to `64`
- `RUNPOD_SERVERLESS_TEMPLATE_ENV` optionally adds template env vars as JSON
- `LUMEN_RUNPOD_CONCURRENCY` defaults to `2`
- `LUMEN_VIDEO_ENCODER` optionally sets FFmpeg encoder, such as `h264_nvenc`

## Vast

Build from the repository root:

```sh
container build -f crates/lumen-server/Dockerfile.vast -t lumen-vast:latest .
```

The `.github/workflows/deploy-vast-template.yml` workflow publishes the Vast
image to GHCR and runs `bun tooling/deploy-vast-template.ts`. Vast templates use
`runtype=args`, publish port `8080`, and default to your requested GPU/price
filters: RTX 5070 Ti through 5090, RTX 4070 Ti through 4090, L4, RTX 4000-6000
Ada, and `dph_total <= 0.70`.

Required GitHub secrets:

- `VAST_API_KEY`

Optional GitHub secrets and variables:

- `LUMEN_VAST_API_TOKEN` protects `POST /render` with `Authorization: Bearer`
- `VAST_TEMPLATE_HASH_ID` updates an existing template when set
- `VAST_TEMPLATE_NAME` defaults to `lumen-render-vast`
- `VAST_TEMPLATE_MAX_DPH_TOTAL` defaults to `0.70`
- `VAST_TEMPLATE_DISK_GB` defaults to `64`
- `VAST_TEMPLATE_EXTRA_FILTERS` overrides the default Vast search filters
- `LUMEN_VIDEO_ENCODER` optionally sets FFmpeg encoder, such as `h264_nvenc`

For local macOS GPU checks, build `lumen-runpod` or `lumen-vast` with
`--features metal` instead.
