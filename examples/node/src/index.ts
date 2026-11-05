import { readFile, writeFile } from "node:fs/promises";

import { Composition, Lumen, LumenError } from "@lumiscia/lumen-sdk";

const apiKey = process.env.LUMISCIA_API_KEY;
if (!apiKey) {
  throw new Error("Set LUMISCIA_API_KEY to your Lumiscia API key.");
}
const lumen = new Lumen({
  apiKey,
  baseUrl: process.env.LUMISCIA_BASE_URL ?? "https://api.lumiscia.com/v1",
});

const composition = new Composition({
  renderSettings: { width: 1280, height: 720 },
  timeline: { fps: 30, durationSeconds: 4 },
});

const video = composition.addNode({
  type: "media_in",
  params: { kind: "video", source: "clip" },
});
const scaled = composition.addNode({
  type: "resize",
  params: { width: 1280, height: 720, mode: "fill" },
  inputs: { source: video },
});
const title = composition.addNode({
  type: "text",
  params: {
    content: "Hello from Lumen",
    font_family: "Roboto",
    font_size: 96,
    color: [255, 255, 255, 255],
    position: [64, 520],
  },
});
const merged = composition.addNode({
  type: "merge",
  inputs: { base: scaled, overlay: title },
});
composition.addNode({ type: "media_output", inputs: { source: merged } });

composition
  .addAudioTrack({ id: "clip-audio", name: "Clip audio" })
  .addAudio({ source: "clip", start: 0, duration: 4, trackId: "clip-audio" });

const clip = await lumen.uploadMedia({
  body: await readFile(new URL("../../assets/clip.mp4", import.meta.url)),
  fileName: "clip.mp4",
});
const render = await lumen.render(composition, { media: { clip } });
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
