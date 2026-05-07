import {
  BlobSource,
  BufferSource as MediabunnyBufferSource,
  Source,
  UrlSource,
  type BlobSourceOptions,
  type InputFormat,
  type UrlSourceOptions,
} from "mediabunny";

import { toUint8Array } from "./buffer.js";
import { LumenMediaError } from "./errors.js";
import type { MediaSourceInput } from "./types.js";

type SourceReadResult = {
  bytes: Uint8Array;
};

type ReadableSourceInternals = Source & {
  _read(start: number, end: number): Promise<SourceReadResult | null> | SourceReadResult | null;
};

export const DEFAULT_BLOB_CACHE_BYTES = 8 * 1024 * 1024;
export const DEFAULT_URL_CACHE_BYTES = 64 * 1024 * 1024;

export interface MediaSourceDefaults {
  blobCacheBytes: number;
  formats: InputFormat[];
  urlCacheBytes: number;
}

export interface InputSourceResult {
  source: Source;
  owned: boolean;
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
): InputSourceResult {
  if (input instanceof Source) {
    return { source: input, owned: false };
  }

  if (isUrlLike(input)) {
    return {
      source: createUrlSource(input, { maxCacheSize: defaults.urlCacheBytes }),
      owned: true,
    };
  }

  if (isBlob(input)) {
    return {
      source: createBlobSource(input, { maxCacheSize: defaults.blobCacheBytes }),
      owned: true,
    };
  }

  if (isBufferSource(input)) {
    return {
      source: new MediabunnyBufferSource(toUint8Array(input)),
      owned: true,
    };
  }

  if (!input || typeof input !== "object" || !("kind" in input)) {
    throw new LumenMediaError("invalid_source", "media source input is invalid");
  }

  switch (input.kind) {
    case "blob":
      return {
        source: createBlobSource(input.blob, {
          maxCacheSize: defaults.blobCacheBytes,
          ...input.options,
        }),
        owned: true,
      };
    case "buffer":
      return {
        source: new MediabunnyBufferSource(toUint8Array(input.bytes)),
        owned: true,
      };
    case "source":
      return { source: input.source, owned: false };
    case "url":
      return {
        source: createUrlSource(input.url, {
          maxCacheSize: defaults.urlCacheBytes,
          ...input.options,
        }),
        owned: true,
      };
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

  if (input instanceof Source) {
    return sourceToBlob(input);
  }

  if (!input || typeof input !== "object" || !("kind" in input)) {
    throw new LumenMediaError("invalid_source", "image source input is invalid");
  }

  switch (input.kind) {
    case "blob":
      return input.blob;
    case "buffer":
      return bytesToBlob(toUint8Array(input.bytes));
    case "source":
      return sourceToBlob(input.source);
    case "url":
      return fetchBlob(input.url, input.options?.requestInit);
    default:
      throw new LumenMediaError("invalid_source", "image source kind is unsupported");
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

async function sourceToBlob(source: Source): Promise<Blob> {
  const size = await source.getSize();
  const result = await (source as ReadableSourceInternals)._read(0, size);
  if (!result) {
    throw new LumenMediaError("invalid_source", "media source did not return bytes");
  }

  return bytesToBlob(result.bytes);
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
