import { isRecord } from "./errors.js";
import { parseFailure } from "./render.js";
import type { RenderEvent, RenderFailure } from "./types.js";

/**
 * Converts a render socket message into a `RenderEvent`. Accepts both the hosted API shape
 * (`{ type, state: { renderId, progress, artifactUrl, error, ... } }`) and the flat shape sent by
 * `lumen-server`. Returns `undefined` for messages that aren't render events, such as `pong`.
 */
export function normalizeRenderEvent(message: unknown): RenderEvent | undefined {
  if (!isRecord(message) || typeof message.type !== "string") {
    return undefined;
  }

  const fields = isRecord(message.state) ? message.state : message;
  const renderId = typeof fields.renderId === "string" ? fields.renderId : undefined;
  if (renderId === undefined) {
    return undefined;
  }

  switch (message.type) {
    case "render.queued":
      return {
        type: "render.queued",
        renderId,
        ...(typeof fields.position === "number" ? { position: fields.position } : {}),
      };
    case "render.started":
      return { type: "render.started", renderId };
    case "render.progress":
      return {
        type: "render.progress",
        renderId,
        progress: typeof fields.progress === "number" ? fields.progress : 0,
        ...(typeof fields.frame === "number" ? { frame: fields.frame } : {}),
        ...(typeof fields.totalFrames === "number" ? { totalFrames: fields.totalFrames } : {}),
      };
    case "render.completed": {
      const url = stringField(fields.url) ?? stringField(fields.artifactUrl);
      return {
        type: "render.completed",
        renderId,
        ...(url !== undefined ? { url } : {}),
        ...(typeof fields.artifactId === "string" ? { artifactId: fields.artifactId } : {}),
      };
    }
    case "render.failed":
      return { type: "render.failed", renderId, error: renderFailure(fields.error) };
    default:
      return undefined;
  }
}

function renderFailure(error: unknown): RenderFailure {
  return (
    parseFailure(error) ?? {
      code: "render_failed",
      message: typeof error === "string" && error.length > 0 ? error : "Render failed.",
    }
  );
}

function stringField(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
