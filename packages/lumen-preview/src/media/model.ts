import { toMediaError } from "./errors.js";
import { sourceInputToBlob } from "./source.js";
import type { MediaSourceInput } from "./types.js";

/**
 * A file the engine reads itself: a binary glTF model, an `.hdr` or `.exr` environment map, or a
 * `.cube` LUT.
 */
export type SceneFileKind = "environment" | "lut" | "model";

const NAMES: Record<SceneFileKind, string> = {
  environment: "environment map",
  lut: "LUT",
  model: "model",
};

/** The bytes of the file `source` holds, or a media error naming `id`. */
export async function readSceneFile(
  kind: SceneFileKind,
  id: string,
  source: MediaSourceInput,
  signal?: AbortSignal,
): Promise<Uint8Array> {
  try {
    const blob = await sourceInputToBlob(source, signal);
    return new Uint8Array(await blob.arrayBuffer());
  } catch (error) {
    throw toMediaError(
      "invalid_source",
      id,
      `${NAMES[kind]} source "${id}" failed to load`,
      error,
      {
        wanted: [kind],
      },
    );
  }
}
