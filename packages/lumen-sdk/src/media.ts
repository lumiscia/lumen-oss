import type { Media, MediaKind, MediaMetadata } from "@lumiscia/lumen-types";

import { LumenError, isRecord } from "./errors.js";

function malformed(what: string): LumenError {
  return new LumenError("invalid_response", `Lumen API returned a malformed ${what}.`);
}

function stringOrNull(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

/** Reads media, which a host may send with more fields than `Media` has, or fewer nullable ones. */
export function parseMedia(value: unknown): Media {
  if (!isRecord(value)) {
    throw malformed("media");
  }
  const { id, status, fileName, contentType, createdAt, expiresAt, sizeBytes, sourceType } = value;
  if (
    typeof id !== "string" ||
    typeof status !== "string" ||
    typeof fileName !== "string" ||
    typeof contentType !== "string" ||
    typeof createdAt !== "string" ||
    typeof expiresAt !== "string" ||
    typeof sizeBytes !== "number" ||
    (sourceType !== "upload" && sourceType !== "url")
  ) {
    throw malformed("media");
  }
  return {
    id,
    status,
    fileName,
    contentType,
    createdAt,
    expiresAt,
    sizeBytes,
    sourceType,
    kind: typeof value.kind === "string" ? (value.kind as MediaKind) : null,
    metadata: isRecord(value.metadata) ? (value.metadata as MediaMetadata) : null,
    objectKey: stringOrNull(value.objectKey),
    rejectionReason: stringOrNull(value.rejectionReason),
    sourceUrl: stringOrNull(value.sourceUrl),
    uploadedBytes: typeof value.uploadedBytes === "number" ? value.uploadedBytes : null,
  };
}

/** Reads the response to creating an upload: the media and where to PUT it. */
export function parseUploadTarget(body: Record<string, unknown>): {
  readonly media: Media;
  readonly uploadUrl: string;
} {
  if (typeof body.uploadUrl !== "string") {
    throw malformed("media upload");
  }
  return { media: parseMedia(body.media), uploadUrl: body.uploadUrl };
}

export function parseUploadId(body: Record<string, unknown>): string {
  if (typeof body.uploadId !== "string") {
    throw malformed("multipart upload");
  }
  return body.uploadId;
}

export function parsePart(body: Record<string, unknown>): {
  readonly etag: string;
  readonly partNumber: number;
} {
  if (typeof body.etag !== "string" || typeof body.partNumber !== "number") {
    throw malformed("upload part");
  }
  return { etag: body.etag, partNumber: body.partNumber };
}
