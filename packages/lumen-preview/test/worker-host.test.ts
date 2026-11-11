import type { MediaInventory, MediaSourceKind } from "@lumiscia/lumen-types";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LumenPreviewBindings } from "../src/runtime.js";
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

const video = vi.hoisted(() => ({
  disposed: 0,
  /** Holds back every frame decode until it resolves, when set. */
  frames: null as Promise<void> | null,
  metadata: new Map<string, Promise<void>>(),
  opened: 0,
}));

// Opening a video and decoding its frames wait for the test.
vi.mock("../src/media/video-session.js", async (original) => ({
  ...(await original<typeof import("../src/media/video-session.js")>()),
  VideoSession: class {
    constructor(readonly streamId: string) {
      video.opened += 1;
    }
    async loadMetadata() {
      await video.metadata.get(this.streamId);
      return { fps: 30, frameCount: 30, height: 1, width: 1 };
    }
    clearFrames() {}
    async decodeFrames() {
      await video.frames;
      return new Map();
    }
    dispose() {
      video.disposed += 1;
    }
  },
}));

const calls: string[] = [];
const drawn: string[] = [];
const shown: number[] = [];
let gpuReady = deferred();
/** Models the fake wasm store refuses, as it refuses a file that isn't a model. */
const corruptModels = new Set<string>();
/** Sources each composition's frames need video frames of. */
const framesOf = new Map<string, string[]>();
let testCanvas = { height: 0, width: 0 };
/** What the wasm store holds, by kind. */
const stored: Record<"environment" | "image" | "model" | "video", Set<string>> = {
  environment: new Set(),
  image: new Set(),
  model: new Set(),
  video: new Set(),
};

/** Draws whichever composition is loaded once its GPU device is ready, as the wasm controller does. */
class FakeController {
  #composition: string | null = null;
  #frame = 0;
  audioTimeline() {
    return undefined;
  }
  clear() {
    for (const ids of Object.values(stored)) {
      ids.clear();
    }
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
  frameRequirements(frame: number) {
    const videos = (framesOf.get(this.#composition ?? "") ?? []).map((streamId) => ({
      frames: [frame],
      streamId,
    }));
    return JSON.stringify({ images: [], videos });
  }
  hasEnvironment(id: string) {
    return stored.environment.has(id);
  }
  hasImage(id: string) {
    return stored.image.has(id);
  }
  hasModel(id: string) {
    return stored.model.has(id);
  }
  hasVideoFrame() {
    return false;
  }
  height() {
    return this.#composition === "B" ? 2 : 1;
  }
  removeEnvironmentSource(id: string) {
    stored.environment.delete(id);
  }
  removeImageSource(id: string) {
    stored.image.delete(id);
  }
  removeModelSource(id: string) {
    stored.model.delete(id);
  }
  removeVideoSource(id: string) {
    stored.video.delete(id);
  }
  setEnvironment(id: string) {
    calls.push(`environment ${id}`);
    stored.environment.add(id);
  }
  setFrame(frame: number) {
    this.#frame = frame;
  }
  setImage(id: string) {
    calls.push(`image ${id}`);
    stored.image.add(id);
  }
  setLogLevel() {}
  setModel(id: string) {
    if (corruptModels.has(id)) {
      throw Object.assign(new Error(`${id}: not a model`), {
        media: { code: "media.decode", details: "not a model", source: id, wanted: ["model"] },
      });
    }
    calls.push(`model ${id}`);
    stored.model.add(id);
  }
  setVideoMetadata(id: string) {
    calls.push(`video ${id}`);
    stored.video.add(id);
  }
  width() {
    return this.#composition === "B" ? 2 : 1;
  }
  loadComposition(json: string) {
    calls.push(`load ${json}`);
    this.#composition = json;
  }
  async renderNow() {
    await gpuReady.promise;
    drawn.push(this.#composition ?? "nothing");
    shown.push(this.#frame);
  }
}

/** What the engine lists each test composition needing, by its JSON. */
const inventories = new Map<string, MediaInventory>();

function needs(json: string, media: [source: string, kind: MediaSourceKind][]) {
  inventories.set(json, {
    dynamic: [],
    fonts: [],
    media: media.map(([source, kind]) => ({
      kinds: [kind],
      source,
      uses: [{ at: { node: 1, param: "source", type: "node" }, kinds: [kind] }],
    })),
  });
}

const sent: LumenPreviewWorkerEvent[] = [];
let receive: (data: LumenPreviewWorkerCommand) => void = () => {};

function load(loadId: number, compositionJson: string, media: string[] = []) {
  receive({
    clock: { atEpochMs: 0, originMs: 0, rate: 0 },
    compositionJson,
    fonts: [],
    keepFonts: [],
    keepMedia: media,
    loadId,
    media: media.map((source) => [source, { source: `https://a.test/${source}` }]),
    type: "load",
  });
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 5));
const lastMedia = () =>
  sent.filter((event) => event.type === "media").at(-1) as
    | Extract<LumenPreviewWorkerEvent, { type: "media" }>
    | undefined;

async function startWorker() {
  receive({ logLevel: "off", lookaheadCount: 0, type: "start", wasmUrl: "lumen.wasm" });
  await settle();
  receive({ canvas: testCanvas as OffscreenCanvas, type: "attach-canvas" });
}

beforeEach(() => {
  calls.length = 0;
  drawn.length = 0;
  shown.length = 0;
  sent.length = 0;
  inventories.clear();
  corruptModels.clear();
  framesOf.clear();
  testCanvas = { height: 0, width: 0 };
  video.frames = null;
  for (const ids of Object.values(stored)) {
    ids.clear();
  }
  video.disposed = 0;
  video.metadata.clear();
  video.opened = 0;
  gpuReady = deferred();
  vi.stubGlobal("navigator", { gpu: { requestAdapter: async () => ({}) } });
  vi.stubGlobal("postMessage", (event: LumenPreviewWorkerEvent) => sent.push(event));
  vi.stubGlobal("fetch", async () => new Response(new Uint8Array([1])));
  vi.stubGlobal("createImageBitmap", async () => ({ close: () => {}, height: 1, width: 1 }));
  vi.stubGlobal(
    "OffscreenCanvas",
    class {
      getContext() {
        return { drawImage: () => {}, getImageData: () => ({ data: new Uint8ClampedArray(4) }) };
      }
    },
  );
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
      mediaDependencies: (json: string) => {
        calls.push(`inventory ${json}`);
        return inventories.get(json) ?? { dynamic: [], fonts: [], media: [] };
      },
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
    await startWorker();
    needs("B", [["fox", "model"]]);

    load(1, "A");
    await settle();
    load(2, "B", ["fox"]);
    await settle();
    gpuReady.resolve();
    await settle();

    expect(drawn).toEqual(["A"]);
    expect(calls).toEqual(["inventory A", "load A", "inventory B"]);

    fox.resolve(new Response(new Uint8Array([1])));
    await settle();
    expect(calls.slice(3)).toEqual(["model fox", "load B"]);
    expect(drawn.at(-1)).toBe("B");
  });

  for (const kind of ["image", "video", "model", "environment"] as const) {
    it(`holds the last frame shown until a pending ${kind} is ready, then shows the new composition`, async () => {
      const pending = deferred<Response>();
      const opened = deferred();
      if (kind === "video") {
        video.metadata.set("source", opened.promise);
      } else {
        vi.stubGlobal("fetch", () => pending.promise);
      }
      gpuReady.resolve();
      await startWorker();
      needs("B", [["source", kind]]);
      load(1, "A");
      await settle();
      expect(drawn).toEqual(["A"]);

      load(2, "B", ["source"]);
      await settle();
      expect(drawn).toEqual(["A"]);
      expect(lastMedia()).toEqual({
        loadId: 2,
        media: [{ failure: null, kind, source: "source", status: "pending" }],
        type: "media",
      });
      expect(sent.filter((event) => event.type === "loaded")).toHaveLength(1);

      opened.resolve();
      pending.resolve(new Response(new Uint8Array([1])));
      await settle();
      expect(lastMedia()?.media[0]?.status).toBe("ready");
      expect(drawn).toEqual(["A", "B"]);
    });
  }

  it("fails the load with the media failure of a source the composition names and `media` lacks", async () => {
    await startWorker();
    needs("A", [["sky.hdr", "environment"]]);

    load(1, "A");
    await settle();

    expect(sent.find((event) => event.type === "load-failed")).toMatchObject({
      loadId: 1,
      media: { code: "media.not_found", source: "sky.hdr", wanted: ["environment"] },
    });
    expect(calls).toEqual(["inventory A"]);
  });

  it("fails the load with the decode failure of a model that isn't one, and keeps the last frame", async () => {
    gpuReady.resolve();
    await startWorker();
    needs("B", [["fox", "model"]]);
    corruptModels.add("fox");
    load(1, "A");
    await settle();

    load(2, "B", ["fox"]);
    await settle();

    expect(sent.find((event) => event.type === "load-failed")).toMatchObject({
      loadId: 2,
      media: { code: "media.decode", source: "fox", wanted: ["model"] },
    });
    expect(lastMedia()?.media[0]).toMatchObject({ source: "fox", status: "failed" });
    expect(drawn).toEqual(["A"]);
  });

  it("keeps the last frame and its size while a video frame decodes, and says playback waits", async () => {
    gpuReady.resolve();
    await startWorker();
    load(1, "A");
    await settle();
    expect(testCanvas).toEqual({ height: 1, width: 1 });

    const frame = deferred();
    video.frames = frame.promise;
    needs("B", [["clip", "video"]]);
    framesOf.set("B", ["clip"]);
    receive({
      clock: { atEpochMs: performance.timeOrigin + performance.now(), originMs: 0, rate: 1 },
      compositionJson: "B",
      fonts: [],
      keepFonts: [],
      keepMedia: ["clip"],
      loadId: 2,
      media: [["clip", { source: "https://a.test/clip" }]],
      type: "load",
    });
    await new Promise((resolve) => setTimeout(resolve, 150));

    expect(sent.some((event) => event.type === "loaded" && event.loadId === 2)).toBe(true);
    expect(drawn).toEqual(["A"]);
    expect(testCanvas).toEqual({ height: 1, width: 1 });
    expect(sent.filter((event) => event.type === "waiting")).toEqual([
      { loadId: 2, type: "waiting", waiting: true },
    ]);

    frame.resolve();
    await settle();
    expect(drawn.at(-1)).toBe("B");
    expect(testCanvas).toEqual({ height: 2, width: 2 });
    expect(sent.filter((event) => event.type === "waiting").at(-1)).toMatchObject({
      waiting: false,
    });
    // Stops the playing runner, which would otherwise draw on into the next test.
    receive({ loadId: 3, type: "unload" });
  });

  it("releases what a composition stops naming, so swapping doesn't grow what is registered", async () => {
    gpuReady.resolve();
    await startWorker();
    needs("A", [
      ["logo", "image"],
      ["fox", "model"],
      ["shared", "image"],
    ]);
    needs("B", [
      ["clip", "video"],
      ["sky", "environment"],
      ["shared", "image"],
    ]);
    const media = ["logo", "fox", "clip", "sky", "shared"];

    for (let loadId = 1; loadId <= 6; loadId += 1) {
      load(loadId, loadId % 2 ? "A" : "B", media);
      await settle();
      const expected =
        loadId % 2
          ? { environment: [], image: ["logo", "shared"], model: ["fox"], video: [] }
          : { environment: ["sky"], image: ["shared"], model: [], video: ["clip"] };
      expect(
        Object.fromEntries(Object.entries(stored).map(([kind, ids]) => [kind, [...ids].sort()])),
      ).toEqual(expected);
    }

    expect(calls.filter((call) => call === "image shared")).toHaveLength(1);
    expect(video.opened - video.disposed).toBe(1);
    expect(drawn.at(-1)).toBe("B");
  });

  it("asks the page for audio, shows the composition without waiting for it, and releases it", async () => {
    gpuReady.resolve();
    await startWorker();
    needs("A", [["song", "audio"]]);

    load(1, "A", ["song"]);
    await settle();
    const request = sent.find((event) => event.type === "audio-source");
    expect(request).toMatchObject({ input: "https://a.test/song", source: "song" });
    expect(drawn).toEqual(["A"]);
    expect(lastMedia()?.media[0]?.status).toBe("pending");

    receive({
      failure: null,
      requestId: (request as { requestId: number }).requestId,
      type: "audio-source-settled",
    });
    await settle();
    expect(lastMedia()?.media[0]?.status).toBe("ready");

    load(2, "B");
    await settle();
    expect(sent.filter((event) => event.type === "audio-source-released")).toEqual([
      { source: "song", type: "audio-source-released" },
    ]);
  });

  it("plays only the range it's given, and says when a range that doesn't loop has ended", async () => {
    gpuReady.resolve();
    await startWorker();
    receive({
      clock: { atEpochMs: 0, originMs: 0, rate: 0 },
      loop: false,
      range: { in: 10, out: 14 },
      revision: 3,
      type: "set-playback",
    });

    receive({ clock: { atEpochMs: 0, originMs: 0, rate: 0 }, revision: 4, type: "clock" });

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
      { loadId: 1, playbackRevision: 4, seekId: 0, type: "ended" },
    ]);
  });
});
