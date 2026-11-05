import { LumenError, isRecord } from "./errors.js";
import type { CreateMediaUploadResult, MediaUploadPart, TemporaryMedia } from "./types.js";

function malformed(what: string): LumenError {
  return new LumenError("invalid_response", `Lumen API returned a malformed ${what}.`);
}

export function parseMedia(value: unknown): TemporaryMedia {
  if (!isRecord(value) || typeof value.id !== "string") {
    throw malformed("media");
  }
  return value as unknown as TemporaryMedia;
}

export function parseUploadTarget(body: Record<string, unknown>): CreateMediaUploadResult {
  if (typeof body.uploadUrl !== "string") {
    throw malformed("media upload");
  }
  return { ...(body as unknown as CreateMediaUploadResult), media: parseMedia(body.media) };
}

export function parseUploadId(body: Record<string, unknown>): string {
  if (typeof body.uploadId !== "string") {
    throw malformed("multipart upload");
  }
  return body.uploadId;
}

export function parsePart(body: Record<string, unknown>): MediaUploadPart {
  if (typeof body.etag !== "string" || typeof body.partNumber !== "number") {
    throw malformed("upload part");
  }
  return { etag: body.etag, partNumber: body.partNumber };
}
