import { Composition, nodes as n } from "@lumiscia/lumen-sdk";

import { footage } from "./components/footage.js";
import { title } from "./components/title.js";
import { FPS, H, SECONDS, W } from "./theme.js";

/** The clip at `clip`, with its sound, under a title. */
export function createComposition(clip: string): Composition {
  return new Composition({
    renderSettings: { width: W, height: H },
    timeline: { fps: FPS, durationSeconds: SECONDS },
  })
    .addOutput(n.merge({ base: footage(clip), overlay: title("Hello from Lumen") }))
    .addAudio({ source: clip, startSeconds: 0, durationSeconds: SECONDS });
}
