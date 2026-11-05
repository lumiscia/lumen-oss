import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const WASM = readFileSync(new URL("../wasm/lumen_wasm_bg.wasm", import.meta.url));
const COMPOSITION = JSON.stringify({
  timeline: { fps: 30, duration_frames: 90 },
  render_settings: { width: 8, height: 4 },
  nodes: [
    { id: 1, type: "background", params: { width: 8, height: 4, paint: [0, 0, 0, 255] } },
    { id: 2, type: "media_output" },
  ],
  connections: [{ from_node: 1, from_port: "output", to_node: 2, to_port: "source" }],
});

/** Replaces `fetch` with one whose responses wait until a test releases them. */
function holdFetches() {
  const held = [];
  globalThis.fetch = () =>
    new Promise((resolve, reject) => {
      held.push({
        fail: () => reject(new Error("offline")),
        release: () =>
          resolve(
            new Response(WASM, {
              headers: { "Content-Type": "application/wasm" },
            }),
          ),
      });
    });
  return held;
}

async function loadedBindings() {
  return (await import("@lumiscia/lumen-bindings")).createLumenBindings({
    previewWasmUrl: "https://a.test/lumen.wasm",
    previewWorkerUrl: "https://a.test/worker.js",
  });
}

function liveController(Controller) {
  const controller = new Controller();
  controller.loadComposition(COMPOSITION);
  assert.equal(controller.durationFrames(), 90);
  return controller;
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 500));

async function overlappingPreviews() {
  const held = holdFetches();
  const bindings = await loadedBindings();
  const first = bindings.preview();
  const second = bindings.preview();
  await settle();

  held[0].release();
  const controller = liveController((await first).LumenPreviewController);

  for (const fetched of held.slice(1)) {
    fetched.release();
  }
  await second;
  await settle();
  assert.equal(controller.durationFrames(), 90);
}

async function initSyncDuringPreview() {
  const held = holdFetches();
  const bindings = await loadedBindings();
  const { initSync, LumenPreviewController } = await import("@lumiscia/lumen-bindings");
  const pending = bindings.preview();
  await settle();

  initSync({ module: WASM });
  const controller = liveController(LumenPreviewController);

  held[0].release();
  await pending;
  await settle();
  assert.equal(controller.durationFrames(), 90);
}

async function retryAfterFailure() {
  const held = holdFetches();
  const bindings = await loadedBindings();
  const failed = bindings.preview();
  await settle();
  held[0].fail();
  await assert.rejects(failed, /offline/);

  const retried = bindings.preview();
  await settle();
  held[1].release();
  liveController((await retried).LumenPreviewController);
}

// The wasm instance is global to the module, so each scenario runs in its own process.
const scenarios = {
  overlappingPreviews,
  initSyncDuringPreview,
  retryAfterFailure,
};

const scenario = process.argv[2];
if (scenario) {
  await scenarios[scenario]();
} else {
  for (const name of Object.keys(scenarios)) {
    const { status } = spawnSync(process.execPath, [fileURLToPath(import.meta.url), name], {
      stdio: "inherit",
    });
    assert.equal(status, 0, `${name} failed`);
  }
  console.log("@lumiscia/lumen-bindings keeps one wasm instance through overlapping inits");
}
