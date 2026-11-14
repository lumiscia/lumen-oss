import type { RenderFailure } from "@lumiscia/lumen-types";

import { LumenError, isRecord, withMedia } from "./errors.js";
import type { FailedRender, Render, RenderStatus } from "./types.js";

const statuses: readonly RenderStatus[] = [
  "queued",
  "processing",
  "succeeded",
  "failed",
  "cancelled",
];

export function isRenderStatus(value: unknown): value is RenderStatus {
  return statuses.includes(value as RenderStatus);
}

export function isTerminal(
  status: RenderStatus,
): status is Exclude<RenderStatus, "queued" | "processing"> {
  return status === "succeeded" || status === "failed" || status === "cancelled";
}

/**
 * An absolute URL as it is. A relative one, such as lumen-server's `/renders/:id/artifact`, is
 * taken to be under the API base, whether or not the base's path ends in `/`.
 */
export function resolveUrl(url: string, baseUrl: URL): string {
  if (URL.canParse(url)) {
    return new URL(url).href;
  }
  const root = new URL(baseUrl);
  root.pathname = root.pathname.replace(/\/?$/, "/");
  root.search = "";
  root.hash = "";
  return new URL(url.replace(/^\/+/, ""), root).href;
}

/** Reads the `render` object of an API response, resolving a relative output URL against the API. */
export function parseRender(job: unknown, baseUrl: URL, extra: { cached?: boolean } = {}): Render {
  if (!isRecord(job) || typeof job.id !== "string" || !isRenderStatus(job.status)) {
    throw new LumenError("invalid_response", "Lumen API returned a malformed render.");
  }

  const fields = {
    id: job.id,
    progress: typeof job.progress === "number" ? job.progress : 0,
    ...(extra.cached !== undefined ? { cached: extra.cached } : {}),
    ...(typeof job.costMicros === "number" ? { costMicros: job.costMicros } : {}),
    ...(typeof job.createdAt === "string" ? { createdAt: job.createdAt } : {}),
    ...(typeof job.inputHash === "string" ? { inputHash: job.inputHash } : {}),
    ...(typeof job.organizationId === "string" ? { organizationId: job.organizationId } : {}),
    ...(typeof job.durationMs === "number" ? { durationMs: job.durationMs } : {}),
    ...(typeof job.outputBytes === "number" ? { outputBytes: job.outputBytes } : {}),
    ...(typeof job.outputExpiresAt === "string" ? { outputExpiresAt: job.outputExpiresAt } : {}),
  };
  switch (job.status) {
    case "succeeded":
      if (typeof job.url !== "string" || job.url === "") {
        throw new LumenError(
          "invalid_response",
          "Lumen API returned a succeeded render without an output URL.",
        );
      }
      return { ...fields, status: job.status, url: resolveUrl(job.url, baseUrl) };
    case "failed":
    case "cancelled":
      return {
        ...fields,
        status: job.status,
        error: parseFailure(job.error) ?? defaultFailure(job.id, job.status),
      };
    default:
      return { ...fields, status: job.status };
  }
}

/** The error a failed or cancelled render raises where a result was expected. */
export function renderError(render: FailedRender): LumenError {
  const { code, message, retryable, media } = render.error;
  return new LumenError(code, message, {
    renderId: render.id,
    retryable,
    ...(media !== undefined ? { media } : {}),
  });
}

function parseFailure(error: unknown): RenderFailure | undefined {
  if (!isRecord(error) || typeof error.message !== "string") {
    return undefined;
  }
  return {
    code: typeof error.code === "string" ? error.code : "render_failed",
    message: error.message,
    retryable: typeof error.retryable === "boolean" ? error.retryable : false,
    ...withMedia(error.media),
  };
}

function defaultFailure(id: string, status: "failed" | "cancelled"): RenderFailure {
  return status === "cancelled"
    ? { code: "render_cancelled", message: `Render ${id} was cancelled.`, retryable: false }
    : { code: "render_failed", message: `Render ${id} failed.`, retryable: false };
}
