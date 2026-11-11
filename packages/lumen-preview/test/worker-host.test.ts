import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LumenPreviewBindings } from "../src/index.js";
import {
  createLumenPreviewWorkerHost,
  type LumenPreviewWorkerCommand,
  type LumenPreviewWorkerEvent,
} from "../src/worker-host.js";

function deferred<T = void>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const calls: string[] = [];
const drawn: string[] = [];
const shown: number[] = [];
let gpuReady = deferred();

/** Draws whichever composition is loaded once its GPU device is ready, as the wasm controller does. */
class FakeController {
  #composition: string | null = null;
  #frame = 0;
  #models = new Set<string>();
  audioTimeline() {
    return undefined;
  }
  currentFrame() {
    return this.#frame;
  }
  durationFrames() {
    return 30;
  }
  fps() {
    return 30;
  }
  frameRequirements() {
    return JSON.stringify({ images: [], videos: [] });
  }
  hasModel(id: string) {
    return this.#models.has(id);
  }
  height() {
    return 1;
  }
  removeModelSource(id: string) {
    this.#models.delete(id);
  }
  setFrame(frame: number) {
    this.#frame = frame;
  }
  setLogLevel() {}
  width() {
    return 1;
  }
  validateComposition(json: string) {
    calls.push(`validate ${json}`);
  }
  loadComposition(json: string) {
    calls.push(`load ${json}`);
    this.#composition = json;
  }
  setModel(id: string) {
    calls.push(`model ${id}`);
    this.#models.add(id);
  }
  async renderNow() {
    await gpuReady.promise;
    drawn.push(this.#composition ?? "nothing");
    shown.push(this.#frame);
  }
}

const sent: LumenPreviewWorkerEvent[] = [];
let receive: (data: LumenPreviewWorkerCommand) => void = () => {};

function load(loadId: number, compositionJson: string, media: { id: string; source: string }[]) {
  receive({
    clock: { atEpochMs: 0, originMs: 0, rate: 0 },
    compositionJson,
    fonts: [],
    keepFonts: [],
    keepMedia: media.map(({ id }) => id),
    loadId,
    media: media.map((entry) => ({ ...entry, kind: "model" })),
    type: "load",
  });
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

beforeEach(() => {
  calls.length = 0;
  drawn.length = 0;
  shown.length = 0;
  sent.length = 0;
  gpuReady = deferred();
  vi.stubGlobal("navigator", { gpu: { requestAdapter: async () => ({}) } });
  vi.stubGlobal("postMessage", (event: LumenPreviewWorkerEvent) => sent.push(event));
  vi.stubGlobal(
    "addEventListener",
    (_type: string, listener: (event: { data: LumenPreviewWorkerCommand }) => void) => {
      receive = (data) => listener({ data });
    },
  );
  createLumenPreviewWorkerHost({
    initPreview: async () => ({}),
    previewBindings: {
      check: () => [],
      LumenPreviewController: FakeController,
    } as unknown as LumenPreviewBindings,
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("createLumenPreviewWorkerHost", () => {
  it("doesn't let a render still waiting for the GPU draw a composition whose media is loading", async () => {
    const fox = deferred<Response>();
    vi.stubGlobal("fetch", () => fox.promise);
    receive({ logLevel: "off", lookaheadCount: 0, type: "start", wasmUrl: "lumen.wasm" });
    await settle();
    receive({ canvas: {} as OffscreenCanvas, type: "attach-canvas" });

    load(1, "A", []);
    await settle();
    load(2, "B", [{ id: "fox", source: "https://a.test/fox.glb" }]);
    await settle();
    gpuReady.resolve();
    await settle();

    expect(drawn).toEqual(["A"]);
    expect(calls).toEqual(["validate A", "load A", "validate B"]);

    fox.resolve(new Response(new Uint8Array([1])));
    await settle();
    expect(calls.slice(3)).toEqual(["model fox", "load B"]);
    expect(drawn.at(-1)).toBe("B");
  });

  it("plays only the range it's given, and says when a range that doesn't loop has ended", async () => {
    gpuReady.resolve();
    receive({ logLevel: "off", lookaheadCount: 0, type: "start", wasmUrl: "lumen.wasm" });
    await settle();
    receive({ canvas: {} as OffscreenCanvas, type: "attach-canvas" });
    receive({ loop: false, range: { in: 10, out: 14 }, type: "set-playback" });

    // 30 fps: frame 10 starts at 333 ms.
    const startMs = (10 * 1_000) / 30;
    receive({
      clock: { atEpochMs: performance.timeOrigin + performance.now(), originMs: startMs, rate: 1 },
      compositionJson: "A",
      fonts: [],
      keepFonts: [],
      keepMedia: [],
      loadId: 1,
      media: [],
      type: "load",
    });
    await new Promise((resolve) => setTimeout(resolve, 300));

    expect(shown).toEqual([10, 11, 12, 13]);
    expect(sent.filter((event) => event.type === "ended")).toEqual([
      { loadId: 1, seekId: 0, type: "ended" },
    ]);
  });
});
