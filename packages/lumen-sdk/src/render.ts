import { LumenError, isRecord } from "./errors.js";
import type { Render, RenderFailure, RenderStatus } from "./types.js";

const statuses: readonly RenderStatus[] = ["queued", "processing", "succeeded", "failed"];

export function isTerminal(status: RenderStatus): boolean {
  return status === "succeeded" || status === "failed";
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
export function parseRender(
  job: unknown,
  baseUrl: URL,
  extra: { cached?: boolean; progress?: number; error?: RenderFailure } = {},
): Render {
  if (!isRecord(job) || typeof job.id !== "string" || !isStatus(job.status)) {
    throw new LumenError("invalid_response", "Lumen API returned a malformed render.");
  }

  const error = extra.error ?? parseFailure(job.error);
  return {
    id: job.id,
    status: job.status,
    ...(extra.progress !== undefined ? { progress: extra.progress } : {}),
    ...(typeof job.outputUrl === "string" && job.outputUrl !== ""
      ? { url: resolveUrl(job.outputUrl, baseUrl) }
      : {}),
    ...(error !== undefined ? { error } : {}),
    ...(extra.cached !== undefined ? { cached: extra.cached } : {}),
    ...(typeof job.costMicros === "number" ? { costMicros: job.costMicros } : {}),
    ...(typeof job.createdAt === "string" ? { createdAt: job.createdAt } : {}),
    ...(typeof job.inputHash === "string" ? { inputHash: job.inputHash } : {}),
    ...(typeof job.organizationId === "string" ? { organizationId: job.organizationId } : {}),
    ...(typeof job.outputExpiresAt === "string" ? { outputExpiresAt: job.outputExpiresAt } : {}),
  };
}

/** The error a failed render raises where a result was expected. */
export function renderError(render: Render): LumenError {
  const failure = render.error;
  return new LumenError(
    failure?.code ?? "render_failed",
    failure?.message ?? `Render ${render.id} failed.`,
    {
      renderId: render.id,
      ...(failure?.retryable !== undefined ? { retryable: failure.retryable } : {}),
    },
  );
}

export function parseFailure(error: unknown): RenderFailure | undefined {
  if (!isRecord(error) || typeof error.message !== "string") {
    return undefined;
  }
  return {
    code: typeof error.code === "string" ? error.code : "render_failed",
    message: error.message,
    ...(typeof error.retryable === "boolean" ? { retryable: error.retryable } : {}),
  };
}

function isStatus(value: unknown): value is RenderStatus {
  return statuses.includes(value as RenderStatus);
}
