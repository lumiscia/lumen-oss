import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { LumenPreviewController, createLumenBindings } from "@lumiscia/lumen-bindings/node";

const {
  LumenPreviewController: Controller,
  check,
  mediaDependencies,
  modelInfo,
} = await createLumenBindings().preview();
assert.equal(Controller, LumenPreviewController);

const controller = new Controller();
controller.loadComposition(
  JSON.stringify({
    timeline: { fps: 30, duration_frames: 90 },
    render_settings: { width: 8, height: 4 },
    nodes: [
      { id: 1, type: "background", params: { width: 8, height: 4, paint: [0, 0, 0, 255] } },
      { id: 2, type: "media_output" },
    ],
    connections: [{ from_node: 1, from_port: "output", to_node: 2, to_port: "source" }],
  }),
);
assert.equal(controller.durationFrames(), 90);
assert.equal(controller.fps(), 30);
assert.equal(controller.width(), 8);

const diagnostics = check(
  JSON.stringify({
    timeline: { fps: 30, duration_frames: 90 },
    render_settings: { width: 8, height: 4 },
    nodes: [
      { id: 1, type: "text", params: { font_family: "Nope Sans" } },
      { id: 2, type: "media_output" },
    ],
    connections: [{ from_node: 1, from_port: "output", to_node: 2, to_port: "source" }],
  }),
  [],
);
assert.deepEqual(
  diagnostics.map(({ severity, code, path, node }) => ({ severity, code, path, node })),
  [{ severity: "warning", code: "font.fallback", path: "/nodes/0/params/font_family", node: 1 }],
);
assert.equal(check("{").at(0)?.code, "document.syntax");
assert.throws(
  () => controller.loadComposition("{"),
  (error) => error instanceof Error && error.diagnostics[0]?.code === "document.syntax",
);

const inventory = mediaDependencies(
  JSON.stringify({
    timeline: { fps: 30, duration_frames: 90 },
    render_settings: { width: 8, height: 4 },
    nodes: [
      { id: 1, type: "media_in", params: { kind: "video", source: "clip.mp4" } },
      { id: 2, type: "text", params: { font_family: "Display" } },
      { id: 3, type: "media_output" },
    ],
    connections: [{ from_node: 2, from_port: "output", to_node: 3, to_port: "source" }],
    audio: {
      tracks: [{ id: "t" }],
      clips: [{ id: "a", source_id: "song.mp3", track_id: "t", start_ms: 0, duration_ms: 10 }],
    },
  }),
);
assert.deepEqual(
  inventory.media.map(({ source, kinds }) => ({ source, kinds })),
  [
    { source: "clip.mp4", kinds: ["video"] },
    { source: "song.mp3", kinds: ["audio"] },
  ],
);
assert.deepEqual(inventory.media[0].uses[0].at, { type: "node", node: 1, param: "source" });
assert.deepEqual(inventory.fonts, [
  { family: "Display", uses: [{ node: 2, param: "font_family" }] },
]);
assert.throws(
  () => mediaDependencies("{"),
  (error) => error instanceof Error && error.diagnostics[0]?.code === "document.syntax",
);

const grid = readFileSync(new URL("../../../crates/3d/tests/fixtures/grid.glb", import.meta.url));
const info = modelInfo(new Uint8Array(grid));
assert.equal(info.animations[0]?.name, "bend");
assert.ok(info.animations[0].duration > 0);
assert.equal(info.materials.length, 1);
assert.throws(() => modelInfo(new Uint8Array(8)), /the model could not be read/);

console.log("@lumiscia/lumen-bindings/node loaded the web wasm build");
