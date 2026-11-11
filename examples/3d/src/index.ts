import { mkdir, writeFile } from "node:fs/promises";

import { composition as animatedModel } from "./animated-model.js";
import { composition as layers } from "./layers.js";
import { composition as videoOnModel } from "./video-on-model.js";

const compositions = { layers, "animated-model": animatedModel, "video-on-model": videoOnModel };

await mkdir("compositions", { recursive: true });
for (const [name, composition] of Object.entries(compositions)) {
  await writeFile(`compositions/${name}.json`, `${JSON.stringify(composition, null, 2)}\n`);
  console.log(`Wrote compositions/${name}.json`);
}
