import {
  BlobSource,
  BufferSource as MediabunnyBufferSource,
  UrlSource,
  type BlobSourceOptions,
  type InputFormat,
  type Source,
  type UrlSourceOptions,
} from "mediabunny";

import { toUint8Array } from "./buffer.js";
import { LumenMediaError } from "./errors.js";
import type { MediaSourceInput } from "./types.js";

export const DEFAULT_BLOB_CACHE_BYTES = 8 * 1024 * 1024;
export const DEFAULT_URL_CACHE_BYTES = 64 * 1024 * 1024;

export interface MediaSourceDefaults {
  blobCacheBytes: number;
  formats: InputFormat[];
  urlCacheBytes: number;
}

function isRequest(value: unknown): value is Request {
  return typeof Request !== "undefined" && value instanceof Request;
}

function isBlob(value: unknown): value is Blob {
  return typeof Blob !== "undefined" && value instanceof Blob;
}

function isUrlLike(value: unknown): value is string | URL | Request {
  return typeof value === "string" || value instanceof URL || isRequest(value);
}

function isBufferSource(value: unknown): value is BufferSource {
  return value instanceof ArrayBuffer || ArrayBuffer.isView(value);
}

export function createInputSource(
  input: MediaSourceInput,
  defaults: Pick<MediaSourceDefaults, "blobCacheBytes" | "urlCacheBytes">,
): Source {
  if (isUrlLike(input)) {
    return createUrlSource(input, { maxCacheSize: defaults.urlCacheBytes });
  }

  if (isBlob(input)) {
    return createBlobSource(input, { maxCacheSize: defaults.blobCacheBytes });
  }

  if (isBufferSource(input)) {
    return new MediabunnyBufferSource(toUint8Array(input));
  }

  if (!input || typeof input !== "object" || !("kind" in input)) {
    throw new LumenMediaError("invalid_source", "media source input is invalid");
  }

  switch (input.kind) {
    case "blob":
      return createBlobSource(input.blob, {
        maxCacheSize: defaults.blobCacheBytes,
        ...input.options,
      });
    case "buffer":
      return new MediabunnyBufferSource(toUint8Array(input.bytes));
    case "url":
      return createUrlSource(input.url, {
        maxCacheSize: defaults.urlCacheBytes,
        ...input.options,
      });
    default:
      throw new LumenMediaError("invalid_source", "media source kind is unsupported");
  }
}

export async function sourceInputToBlob(input: MediaSourceInput): Promise<Blob> {
  if (isBlob(input)) {
    return input;
  }

  if (isBufferSource(input)) {
    return bytesToBlob(toUint8Array(input));
  }

  if (isUrlLike(input)) {
    return fetchBlob(input);
  }

  if (!input || typeof input !== "object" || !("kind" in input)) {
    throw new LumenMediaError("invalid_source", "media source input is invalid");
  }

  switch (input.kind) {
    case "blob":
      return input.blob;
    case "buffer":
      return bytesToBlob(toUint8Array(input.bytes));
    case "url":
      return fetchBlob(input.url, input.options?.requestInit);
    default:
      throw new LumenMediaError("invalid_source", "media source kind is unsupported");
  }
}

/**
 * Whether two inputs read the same bytes. URLs compare by address and the `kind` wrappers by what
 * they wrap, so a recreated wrapper doesn't reload. Requests only match themselves, because two
 * with the same address can carry different credentials.
 */
export function sameMediaInput(left: MediaSourceInput, right: MediaSourceInput): boolean {
  const [leftPayload, leftOptions] = mediaInputIdentity(left);
  const [rightPayload, rightOptions] = mediaInputIdentity(right);
  return leftPayload === rightPayload && leftOptions === rightOptions;
}

function mediaInputIdentity(input: MediaSourceInput): [payload: unknown, options?: unknown] {
  if (input instanceof URL) {
    return [input.href];
  }
  if (typeof input !== "object" || !("kind" in input)) {
    return [input];
  }
  switch (input.kind) {
    case "blob":
      return [input.blob, input.options];
    case "buffer":
      return [input.bytes];
    case "url":
      return [input.url instanceof URL ? input.url.href : input.url, input.options];
  }
}

export async function fetchBlob(
  source: string | URL | Request,
  requestInit?: RequestInit,
): Promise<Blob> {
  const response = await fetch(source, requestInit);
  if (!response.ok) {
    throw new LumenMediaError(
      "invalid_source",
      `failed to load media source: ${response.status} ${response.statusText}`,
    );
  }

  return response.blob();
}

function createBlobSource(blob: Blob, options: BlobSourceOptions): BlobSource {
  return new BlobSource(blob, options);
}

function createUrlSource(url: string | URL | Request, options: UrlSourceOptions): UrlSource {
  return new UrlSource(url, options);
}

function bytesToBlob(bytes: Uint8Array): Blob {
  const buffer = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(buffer).set(bytes);
  return new Blob([buffer]);
}
