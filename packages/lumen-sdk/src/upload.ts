import { LumenError } from "./errors.js";
import type { UploadMediaBody } from "./types.js";

/** Bodies larger than this, and every stream, go up in parts. */
export const singleRequestLimitBytes = 64 * 1024 * 1024;
export const partSizeBytes = 8 * 1024 * 1024;

const contentTypes = new Map(
  Object.entries({
    aac: "audio/aac",
    avi: "video/x-msvideo",
    avif: "image/avif",
    bmp: "image/bmp",
    flac: "audio/flac",
    gif: "image/gif",
    heic: "image/heic",
    jpeg: "image/jpeg",
    jpg: "image/jpeg",
    m4a: "audio/mp4",
    m4v: "video/mp4",
    mkv: "video/x-matroska",
    mov: "video/quicktime",
    mp3: "audio/mpeg",
    mp4: "video/mp4",
    mpeg: "video/mpeg",
    mpg: "video/mpeg",
    oga: "audio/ogg",
    ogg: "audio/ogg",
    ogv: "video/ogg",
    opus: "audio/opus",
    png: "image/png",
    svg: "image/svg+xml",
    tif: "image/tiff",
    tiff: "image/tiff",
    wav: "audio/wav",
    webm: "video/webm",
    webp: "image/webp",
  }),
);

/** The content type of a common media file name, from its extension. */
export function contentTypeOf(fileName: string): string | undefined {
  return contentTypes.get(fileName.slice(fileName.lastIndexOf(".") + 1).toLowerCase());
}

export function sizeOf(body: Exclude<UploadMediaBody, ReadableStream<Uint8Array>>): number {
  return body instanceof Blob ? body.size : body.byteLength;
}

/**
 * Splits a body into parts of `partSizeBytes`, the last one shorter. A stream's parts share one
 * buffer, so a part is only valid until the next one is requested. If the consumer stops early, or
 * `signal` aborts, a stream is cancelled and unlocked.
 */
export async function* partsOf(
  body: UploadMediaBody,
  sizeBytes: number,
  signal: AbortSignal | undefined,
): AsyncGenerator<Blob | Uint8Array<ArrayBuffer>> {
  if (body instanceof Blob) {
    for (let offset = 0; offset < body.size; offset += partSizeBytes) {
      yield body.slice(offset, offset + partSizeBytes);
    }
  } else if (body instanceof ReadableStream) {
    yield* streamParts(body, sizeBytes, signal);
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
  signal: AbortSignal | undefined,
): AsyncGenerator<Uint8Array<ArrayBuffer>> {
  const reader = stream.getReader();
  const part = new Uint8Array(partSizeBytes);
  let filled = 0;
  let total = 0;
  let finished = false;

  try {
    for (;;) {
      const { done, value } = await abortable(reader.read(), signal);
      if (done) {
        break;
      }
      total += value.byteLength;
      if (total > sizeBytes) {
        throw new LumenError(
          "invalid_upload",
          `The stream produced more than the ${sizeBytes} bytes of sizeBytes.`,
        );
      }
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

    if (total !== sizeBytes) {
      throw new LumenError(
        "invalid_upload",
        `The stream produced ${total} bytes, but sizeBytes is ${sizeBytes}.`,
      );
    }
    finished = true;
  } finally {
    if (!finished) {
      void reader.cancel().catch(() => {});
    }
    reader.releaseLock();
  }

  if (filled > 0) {
    yield part.subarray(0, filled);
  }
}

/** Rejects with the signal's reason as soon as it aborts, without waiting for `promise`. */
function abortable<T>(promise: Promise<T>, signal: AbortSignal | undefined): Promise<T> {
  if (signal === undefined) {
    return promise;
  }
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(signal.reason);
      return;
    }
    const onAbort = () => reject(signal.reason);
    signal.addEventListener("abort", onAbort, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", onAbort));
  });
}
