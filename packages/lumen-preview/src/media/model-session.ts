import { toMediaError } from "./errors.js";
import { sourceInputToBlob } from "./source.js";
import type { MediaSourceInput } from "./types.js";

export class ModelSession {
  private bytesPromise: Promise<Uint8Array> | null = null;

  constructor(
    readonly modelId: string,
    private readonly source: MediaSourceInput,
  ) {}

  /** Joins a read in flight. Nothing is kept after it, so a failed load can be retried. */
  load(): Promise<Uint8Array> {
    this.bytesPromise ??= this.read().finally(() => {
      this.bytesPromise = null;
    });
    return this.bytesPromise;
  }

  private async read(): Promise<Uint8Array> {
    try {
      const blob = await sourceInputToBlob(this.source);
      return new Uint8Array(await blob.arrayBuffer());
    } catch (error) {
      throw toMediaError(
        "invalid_source",
        this.modelId,
        `model source "${this.modelId}" failed to load`,
        error,
      );
    }
  }
}
