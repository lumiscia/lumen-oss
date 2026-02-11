# Runpod Render Runbook

## Overview
This project uses `apps/render` as the `/renders` control plane and Runpod queue endpoints for GPU execution.

## Queue and Retry Behavior
- Queue: `webapp-render-events-queue`
- DLQ: `webapp-render-events-dlq`
- Consumer retries: up to 8
- Upstream retry classes:
  - Retryable: network/timeout, HTTP `408`, `409`, `425`, `429`, `5xx`
  - Non-retryable: HTTP `400`, `401`, `403`, `404`, validation failures

## Webhook Idempotency
- Webhook events are deduplicated via KV keys prefixed with `render:webhook:` and TTL.
- Duplicate events are acknowledged without reprocessing.

## DLQ Replay
1. Inspect failed payloads in `webapp-render-events-dlq`.
2. Verify the target job state in KV (`render:job:{job_id}`).
3. Re-enqueue only if the job is non-terminal or explicitly retried.
4. Use original message type and increment attempt where appropriate.

## Health Checks
- Worker health endpoint: `GET /health`
- Job status endpoint: `GET /renders/{job_id}`

## Common Failure Modes
- `runpod_http_error`: Check Runpod API key, endpoint ID, and rate limits.
- `artifact_download_failed`: Check Runpod output URL validity and artifact retention window.
- `artifact_upload_failed`: Check R2 bucket binding and write permissions.
