import { Composition, Lumen, mediaReference } from "@lumiscia/lumen-sdk";

const mediaUrl =
  process.env.LUMEN_EXAMPLE_MEDIA_URL ??
  "https://images.unsplash.com/photo-1546069901-ba9599a7e63c?w=1280&h=720&fit=crop";
const baseUrl = process.env.LUMEN_BASE_URL ?? "https://lumiscia.com/api/v1";
const mode = process.env.LUMEN_EXAMPLE_MODE ?? (baseUrl.includes("localhost") ? "local" : "cloud");
const textContent = process.env.LUMEN_EXAMPLE_TEXT ?? "Hello from Lumen";
const fontFamily = process.env.LUMEN_EXAMPLE_FONT_FAMILY ?? "Inter";

const lumen = new Lumen({
  apiKey: process.env.LUMEN_API_KEY ?? "dev-api-key",
  baseUrl,
});

const plateAlias = "plate";
let media = mediaUrl;
if (mode !== "local") {
  const plate = await lumen.createUrlMedia({
    fileName: "plate.png",
    url: mediaUrl,
  });
  console.log("Created media:", plate.id, plate.status);
  media = mediaReference(plate);
}

const composition = new Composition({
  metadata: {
    name: "SDK smoke test",
  },
  renderSettings: {
    width: 1920,
    height: 1080,
    background_color: [12, 12, 16, 255],
  },
  timeline: {
    fps: 24,
    durationSeconds: 5,
  },
});

const background = composition.addSolidColor({
  width: 1920,
  height: 1080,
  color: [24, 28, 36, 255],
});

const title = composition.addText({
  content: textContent,
  fontFamily,
  fontSize: 96,
  fontWeight: 700,
  color: [255, 255, 255, 255],
  maxWidth: 1280,
});

const image = composition.addImage(plateAlias, {
  rangeStart: 0,
  rangeEnd: 120,
  loop: true,
});

const merge = composition.addNode({
  type: "merge",
  properties: {
    opacity: 0.9,
    blend_mode: 0,
  },
});

composition.connect(background, merge, { toPort: "base" });
composition.connect(image, merge, { toPort: "overlay" });
composition.connect(title, merge, { toPort: "overlay" });
composition.addOutput(merge);

console.log(JSON.stringify(composition.toJSON(), null, 2));

const { id, error } = await lumen.render(composition, {
  media: {
    [plateAlias]: media,
  },
});
if (error) {
  console.error(error);
} else if (id) {
  console.log("Created render:", id);
  await lumen.waitForRender(id, { onEvent: console.log });
  const artifact = await lumen.getRenderArtifact(id);
  console.log("Artifact:", artifact.type, artifact.size, "bytes");
}
