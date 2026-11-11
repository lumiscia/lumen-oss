import { toMediaError } from "./errors.js";
import { sourceInputToBlob } from "./source.js";
import type { MediaSourceInput } from "./types.js";

/** The bytes of the model file `source` holds, or a media error naming `modelId`. */
export async function readModel(modelId: string, source: MediaSourceInput): Promise<Uint8Array> {
  try {
    const blob = await sourceInputToBlob(source);
    return new Uint8Array(await blob.arrayBuffer());
  } catch (error) {
    throw toMediaError(
      "invalid_source",
      modelId,
      `model source "${modelId}" failed to load`,
      error,
      { wanted: ["model"] },
    );
  }
}
