import { readFile, writeFile } from "node:fs/promises";

import { Lumen, LumenError, assetSource } from "@lumiscia/lumen-sdk";

import { createComposition } from "./composition.js";

const apiKey = process.env.LUMISCIA_API_KEY;
if (!apiKey) {
  throw new Error("Set LUMISCIA_API_KEY to your Lumiscia API key.");
}
const lumen = new Lumen({
  apiKey,
  baseUrl: process.env.LUMISCIA_BASE_URL ?? "https://api.lumiscia.com/v1",
});

const clip = await lumen.uploadMedia({
  body: await readFile(new URL("../../assets/clip.mp4", import.meta.url)),
  fileName: "clip.mp4",
});

const render = await lumen.render(createComposition(assetSource(clip)));
try {
  const done = await lumen.waitForRender(render, {
    onProgress: ({ progress }) => console.log(`${Math.round(progress * 100)}%`),
  });
  const output = await lumen.download(done);
  await writeFile("out.mp4", Buffer.from(await output.arrayBuffer()));
  console.log("Wrote out.mp4");
} catch (error) {
  if (error instanceof LumenError && error.renderId) {
    console.error(`Render ${error.renderId} failed: ${error.code}: ${error.message}`);
  }
  throw error;
}
