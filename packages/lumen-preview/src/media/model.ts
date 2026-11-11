import { toMediaError } from "./errors.js";
import { sourceInputToBlob } from "./source.js";
import type { MediaSourceInput } from "./types.js";

/** A file the engine reads itself: a binary glTF model, or an `.hdr` or `.exr` environment map. */
export type SceneFileKind = "environment" | "model";

const NAMES: Record<SceneFileKind, string> = { environment: "environment map", model: "model" };

/** The bytes of the file `source` holds, or a media error naming `id`. */
export async function readSceneFile(
  kind: SceneFileKind,
  id: string,
  source: MediaSourceInput,
): Promise<Uint8Array> {
  try {
    const blob = await sourceInputToBlob(source);
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
