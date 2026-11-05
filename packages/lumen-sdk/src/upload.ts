import { LumenError } from "./errors.js";
import type { UploadMediaBody } from "./types.js";

/** Bodies larger than this, and every stream, go up in parts. */
export const singleRequestLimitBytes = 64 * 1024 * 1024;
export const partSizeBytes = 8 * 1024 * 1024;

export function sizeOf(body: Exclude<UploadMediaBody, ReadableStream<Uint8Array>>): number {
  return body instanceof Blob ? body.size : body.byteLength;
}

/**
 * Splits a body into parts of `partSizeBytes`, the last one shorter. A stream's parts share one
 * buffer, so a part is only valid until the next one is requested.
 */
export async function* partsOf(
  body: UploadMediaBody,
  sizeBytes: number,
): AsyncGenerator<Blob | Uint8Array<ArrayBuffer>> {
  if (body instanceof Blob) {
    for (let offset = 0; offset < body.size; offset += partSizeBytes) {
      yield body.slice(offset, offset + partSizeBytes);
    }
  } else if (body instanceof ReadableStream) {
    yield* streamParts(body, sizeBytes);
  } else {
    const bytes = ArrayBuffer.isView(body)
      ? new Uint8Array(body.buffer as ArrayBuffer, body.byteOffset, body.byteLength)
      : new Uint8Array(body as ArrayBuffer);
    for (let offset = 0; offset < bytes.byteLength; offset += partSizeBytes) {
      yield bytes.subarray(offset, offset + partSizeBytes);
    }
  }
}

async function* streamParts(
  stream: ReadableStream<Uint8Array>,
  sizeBytes: number,
): AsyncGenerator<Uint8Array<ArrayBuffer>> {
  const reader = stream.getReader();
  const part = new Uint8Array(partSizeBytes);
  let filled = 0;
  let total = 0;

  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      total += value.byteLength;
      for (let offset = 0; offset < value.byteLength;) {
        const copied = Math.min(partSizeBytes - filled, value.byteLength - offset);
        part.set(value.subarray(offset, offset + copied), filled);
        filled += copied;
        offset += copied;
        if (filled === partSizeBytes) {
          yield part;
          filled = 0;
        }
      }
    }
  } finally {
    reader.releaseLock();
  }

  if (total !== sizeBytes) {
    throw new LumenError(
      "invalid_upload",
      `The stream produced ${total} bytes, but sizeBytes is ${sizeBytes}.`,
    );
  }
  if (filled > 0) {
    yield part.subarray(0, filled);
  }
}
