import { mkdir, writeFile } from "node:fs/promises";

import { createAnimatedModel } from "./compositions/animated-model.js";
import { createLayers } from "./compositions/layers.js";
import { createVideoOnModel } from "./compositions/video-on-model.js";

const compositions = {
  layers: createLayers,
  "animated-model": createAnimatedModel,
  "video-on-model": createVideoOnModel,
};

await mkdir("compositions", { recursive: true });
for (const [name, create] of Object.entries(compositions)) {
  await writeFile(`compositions/${name}.json`, `${JSON.stringify(create(), null, 2)}\n`);
  console.log(`Wrote compositions/${name}.json`);
}
