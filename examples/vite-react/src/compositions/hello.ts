import { Composition, nodes as n } from "@lumiscia/lumen-shared";

import { footage } from "../components/footage.js";
import { title } from "../components/title.js";
import { CLIP, FPS, H, SECONDS, W } from "../theme.js";

/** The clip, with its sound, under a title. */
export function createHello(): Composition {
  return new Composition({
    renderSettings: { width: W, height: H },
    timeline: { fps: FPS, durationSeconds: SECONDS },
  })
    .addOutput(n.merge({ base: footage(CLIP), overlay: title("Hello from Lumen") }))
    .addAudio({ source: CLIP, startSeconds: 0, durationSeconds: SECONDS });
}
