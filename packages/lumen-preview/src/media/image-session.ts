import { decodeImageBitmapToRgba } from "./canvas.js";
import { toMediaError } from "./errors.js";
import { sourceInputToBlob } from "./source.js";
import type { DecodedImageFrame, MediaSourceInput } from "./types.js";

export class ImageSession {
  private framePromise: Promise<DecodedImageFrame> | null = null;

  constructor(
    readonly imageId: string,
    private readonly source: MediaSourceInput,
  ) {}

  /** Joins a decode in flight. Nothing is kept after it, so a failed load can be retried. */
  load(): Promise<DecodedImageFrame> {
    this.framePromise ??= this.decode().finally(() => {
      this.framePromise = null;
    });
    return this.framePromise;
  }

  private async decode(): Promise<DecodedImageFrame> {
    try {
      const blob = await sourceInputToBlob(this.source);
      return decodeImageBitmapToRgba(await createImageBitmap(blob));
    } catch (error) {
      throw toMediaError(
        "decode_failed",
        this.imageId,
        `image source "${this.imageId}" failed to decode`,
        error,
        { wanted: ["image"] },
      );
    }
  }
}
