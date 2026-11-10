import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LumenComposition } from "@lumiscia/lumen-types";

import { clockPositionMs } from "../../src/clock.js";
import { LumenPreviewContext } from "../../src/preview.js";
import { LumenPreviewSession } from "../../src/session/session.js";
import type { LumenPreviewWorkerCommand, LumenPreviewWorkerEvent } from "../../src/worker-host.js";

const audio = vi.hoisted(() => ({
  clockRunning: false,
  instances: [] as {
    loadAudioTimeline: ReturnType<typeof vi.fn>;
    options: { onClockChange: () => void };
    pause: ReturnType<typeof vi.fn>;
    play: ReturnType<typeof vi.fn>;
    seekMs: ReturnType<typeof vi.fn>;
  }[],
  playing: false,
  positionMs: 0,
}));

// Follows the engine: an identical timeline is left playing, only clips play, and the position wraps
// at the timeline's end.
vi.mock("../../src/audio-engine.js", () => ({
  LumenAudioEngine: class {
    timeline: { clips: unknown[]; durationFrames: number; fps: number } | null = null;
    timelineJson: string | null = null;
    loadAudioTimeline = vi.fn((json: string | null) => {
      if (json === this.timelineJson) {
        return false;
      }
      this.timelineJson = json;
      this.timeline = json === null ? null : JSON.parse(json);
      audio.playing = false;
      audio.positionMs = 0;
      return true;
    });
    play = vi.fn((ms?: number) => {
      audio.positionMs = this.wrap(ms ?? audio.positionMs);
      audio.playing = (this.timeline?.clips.length ?? 0) > 0;
    });
    constructor(readonly options: { onClockChange: () => void }) {
      audio.instances.push(this);
    }
    currentTimeMs = () => this.wrap(audio.positionMs);
    dispose() {}
    isClockRunning = () => audio.clockRunning;
    isPlaying = () => audio.playing;
    pause = vi.fn(() => {
      audio.playing = false;
    });
    seekMs = vi.fn((ms: number) => {
      audio.positionMs = this.wrap(ms);
    });
    syncAudioSources = async () => {};
    wrap(ms: number) {
      const durationMs = this.timeline
        ? (this.timeline.durationFrames * 1_000) / this.timeline.fps
        : 0;
      return durationMs > 0 ? ms % durationMs : ms;
    }
  },
}));

class FakeWorker {
  static instances: FakeWorker[] = [];
  onerror: ((event: { message: string }) => void) | null = null;
  onmessage: ((event: { data: LumenPreviewWorkerEvent }) => void) | null = null;
  onmessageerror: (() => void) | null = null;
  posted: LumenPreviewWorkerCommand[] = [];
  terminated = false;

  constructor() {
    FakeWorker.instances.push(this);
  }
  postMessage(message: LumenPreviewWorkerCommand) {
    this.posted.push(message);
  }
  terminate() {
    this.terminated = true;
  }
  emit(data: LumenPreviewWorkerEvent) {
    this.onmessage?.({ data });
  }
  ofType<TType extends LumenPreviewWorkerCommand["type"]>(type: TType) {
    return this.posted.filter(
      (message): message is Extract<LumenPreviewWorkerCommand, { type: TType }> =>
        message.type === type,
    );
  }
}

class FakeCanvas {
  removed = false;
  style = {};
  remove() {
    this.removed = true;
  }
  transferControlToOffscreen() {
    return {};
  }
}

/** What the engine serializes, which includes the length, so a new length is a new timeline. */
function audioTimeline(totalFrames: number, clip: string | null = "music", fps = 10): string {
  return JSON.stringify({
    clips: clip === null ? [] : [{ id: clip }],
    durationFrames: totalFrames,
    fps,
    tracks: [],
  });
}

function composition(name: string): LumenComposition {
  return { metadata: { name } } as unknown as LumenComposition;
}

function createSession(inputs: ConstructorParameters<typeof LumenPreviewSession>[0] | object = {}) {
  const preview = new LumenPreviewContext();
  const canvases: unknown[] = [];
  const host = { append: (canvas: unknown) => canvases.push(canvas) } as unknown as HTMLElement;
  const session = new LumenPreviewSession({
    wasmUrl: "https://a.test/lumen.wasm",
    workerUrl: "https://a.test/worker.js",
    composition: composition("first"),
    preview,
    ...inputs,
  });
  const attached = session.attach(host);
  const worker = FakeWorker.instances.at(-1) as FakeWorker;
  return { attached, canvases, preview, session, worker };
}

async function started(inputs?: object) {
  const fake = createSession(inputs);
  fake.worker.emit({ type: "ready" });
  await fake.attached;
  return fake;
}

async function playable(inputs?: object, audioTimelineJson: string | null = audioTimeline(100)) {
  const fake = await started(inputs);
  const { loadId } = fake.worker.ofType("load").at(-1) as { loadId: number };
  fake.worker.emit({
    audioTimelineJson,
    fps: 10,
    height: 720,
    loadId,
    totalFrames: 100,
    type: "loaded",
    width: 1280,
  });
  return fake;
}

describe("LumenPreviewSession", () => {
  beforeEach(() => {
    FakeWorker.instances = [];
    audio.clockRunning = false;
    audio.instances = [];
    audio.playing = false;
    audio.positionMs = 0;
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("Worker", FakeWorker);
    vi.stubGlobal("OffscreenCanvas", class {});
    vi.stubGlobal("HTMLCanvasElement", FakeCanvas);
    vi.stubGlobal("navigator", { gpu: {} });
    vi.stubGlobal("location", { origin: "https://a.test" });
    vi.stubGlobal("origin", "https://a.test");
    vi.stubGlobal("document", {
      baseURI: "https://a.test/",
      createElement: () => new FakeCanvas(),
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("waits for the worker before it creates and transfers a canvas", async () => {
    const { attached, canvases, worker } = createSession();
    expect(worker.posted.map((message) => message.type)).toEqual(["start"]);
    expect(canvases).toHaveLength(0);

    worker.emit({ type: "ready" });
    await attached;
    expect(canvases).toHaveLength(1);
    expect(worker.posted.map((message) => message.type)).toEqual([
      "start",
      "attach-canvas",
      "load",
    ]);
  });

  it("reports a worker that can't start, and leaves the host alone", async () => {
    const { attached, canvases, preview, worker } = createSession();
    worker.onerror?.({ message: "" });
    await attached;

    expect(preview.getSnapshot().errorCode).toBe("worker_failed");
    expect(preview.getSnapshot().error).toContain("worker-src");
    expect(preview.getSnapshot().error).toContain("from https://a.test/worker.js");
    expect(preview.getSnapshot().error).toContain("pass `workerUrl` and `wasmUrl`");
    expect(canvases).toHaveLength(0);
    expect(worker.terminated).toBe(true);
  });

  it("names the Vite setting when a pre-bundled worker URL doesn't load", async () => {
    const workerUrl = "https://a.test/node_modules/.vite/deps/dist/browser/worker.js";
    const { attached, preview, worker } = createSession({ workerUrl });
    worker.onerror?.({ message: "" });
    await attached;

    expect(preview.getSnapshot().error).toContain("optimizeDeps");
  });

  it("passes on the reason the worker gives for not starting", async () => {
    const { attached, preview, worker } = createSession();
    worker.emit({ code: "webgpu_unavailable", message: "no adapter", type: "startup-failed" });
    await attached;
    expect(preview.getSnapshot()).toMatchObject({
      error: "no adapter",
      errorCode: "webgpu_unavailable",
    });
  });

  it("stops a worker still starting as soon as it's disposed", async () => {
    vi.useFakeTimers();
    const { attached, preview, session, worker } = createSession();

    session.dispose();
    expect(worker.terminated).toBe(true);
    await attached;
    expect(vi.getTimerCount()).toBe(0);
    expect(preview.getSnapshot().error).toBeNull();
    vi.useRealTimers();
  });

  it("stops the startup it replaces, when attached again", async () => {
    const { attached, canvases, preview, session, worker } = createSession();
    const replacement = session.attach({
      append: (canvas: unknown) => canvases.push(canvas),
    } as unknown as HTMLElement);
    const nextWorker = FakeWorker.instances.at(-1) as FakeWorker;

    expect(worker.terminated).toBe(true);
    await attached;
    expect(preview.getSnapshot().error).toBeNull();

    nextWorker.emit({ type: "ready" });
    await replacement;
    expect(canvases).toHaveLength(1);
    expect(nextWorker.terminated).toBe(false);
  });

  it("applies what changed while the worker was starting", async () => {
    const { attached, session, worker } = createSession({ lookaheadCount: 8, logLevel: "off" });
    expect(worker.ofType("start")[0]).toMatchObject({ logLevel: "off", lookaheadCount: 8 });

    session.update({ composition: composition("second"), logLevel: "warn", lookaheadCount: 2 });
    expect(worker.ofType("set-options")).toHaveLength(0);
    worker.emit({ type: "ready" });
    await attached;

    expect(worker.ofType("set-options")).toMatchObject([{ logLevel: "warn", lookaheadCount: 2 }]);
    expect(worker.ofType("load")).toHaveLength(1);
    expect(worker.ofType("load")[0]?.compositionJson).toContain("second");
    const types = worker.posted.map((message) => message.type);
    expect(types.indexOf("set-options")).toBeLessThan(types.indexOf("load"));
  });

  it("sends options only when they really change", async () => {
    const { session, worker } = await started({ lookaheadCount: 8 });
    session.update({ composition: composition("first") });
    expect(worker.ofType("set-options")).toHaveLength(0);

    session.update({ composition: composition("first"), lookaheadCount: 2 });
    expect(worker.ofType("set-options")).toMatchObject([{ lookaheadCount: 2 }]);
  });

  it("reports a browser without WebGPU before it creates a worker", async () => {
    vi.stubGlobal("navigator", {});
    const preview = new LumenPreviewContext();
    const session = new LumenPreviewSession({ preview, wasmUrl: "", workerUrl: "" });
    await session.attach({} as HTMLElement);

    expect(preview.getSnapshot().errorCode).toBe("webgpu_unavailable");
    expect(FakeWorker.instances).toHaveLength(0);
  });

  it("removes the canvas and stops playing when the worker crashes", async () => {
    const { canvases, preview, worker } = await playable();
    preview.play();
    worker.onerror?.({ message: "out of memory" });

    expect((canvases[0] as FakeCanvas).removed).toBe(true);
    expect(worker.terminated).toBe(true);
    expect(preview.getSnapshot()).toMatchObject({
      errorCode: "worker_failed",
      isLoaded: false,
      isPlaying: false,
    });
    expect(preview.getSnapshot().error).toContain("out of memory");
  });

  it("holds the worker's clock until the audio moves, then follows it", async () => {
    const { preview, worker } = await playable();
    preview.play();
    expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 0, rate: 0 });

    audio.clockRunning = true;
    audio.positionMs = 40;
    audio.instances[0]?.options.onClockChange();
    expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 40, rate: 1 });

    audio.clockRunning = false;
    audio.instances[0]?.options.onClockChange();
    expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 40, rate: 0 });
  });

  describe("updating the composition", () => {
    function loaded(
      worker: FakeWorker,
      totalFrames: number,
      audioTimelineJson: string | null = audioTimeline(totalFrames),
      fps = 10,
    ) {
      worker.emit({
        audioTimelineJson,
        fps,
        height: 720,
        loadId: worker.ofType("load").at(-1)?.loadId as number,
        totalFrames,
        type: "loaded",
        width: 1280,
      });
    }

    it("keeps playing through the load, and the audio isn't touched when its timeline is the same", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      void preview.seek(30);
      preview.play();
      const calls = (engine?.play.mock.calls.length ?? 0) + (engine?.seekMs.mock.calls.length ?? 0);

      session.update({ composition: composition("second") });
      expect(preview.getSnapshot()).toMatchObject({ frame: 30, isPlaying: true });
      expect(worker.ofType("load").at(-1)?.clock).toMatchObject({ originMs: 3_000 });

      audio.clockRunning = true;
      audio.positionMs = 3_200;
      loaded(worker, 100);
      expect(preview.getSnapshot()).toMatchObject({ frame: 30, isPlaying: true, totalFrames: 100 });
      expect(worker.ofType("seek")).toHaveLength(1);
      expect(worker.posted.at(-1)).toMatchObject({
        clock: { originMs: 3_200, rate: 1 },
        type: "clock",
      });
      expect(engine?.pause).not.toHaveBeenCalled();
      expect((engine?.play.mock.calls.length ?? 0) + (engine?.seekMs.mock.calls.length ?? 0)).toBe(
        calls,
      );
    });

    it("leaves a worker slightly ahead of the audio where it is", async () => {
      const { preview, session, worker } = await started();
      loaded(worker, 300, audioTimeline(300, "music", 30), 30);
      const loadId = worker.ofType("load").at(-1)?.loadId as number;
      preview.play();
      audio.clockRunning = true;
      audio.positionMs = 1_060;
      worker.emit({ actualFps: 30, frame: 32, loadId, renderMs: 1, seekId: 0, type: "frame" });

      session.update({ composition: composition("colour") });
      loaded(worker, 300, audioTimeline(300, "music", 30), 30);

      expect(preview.getSnapshot()).toMatchObject({ frame: 32, isPlaying: true });
      expect(worker.ofType("seek")).toEqual([]);
      expect(worker.posted.at(-1)).toMatchObject({
        clock: { originMs: 1_060, rate: 1 },
        type: "clock",
      });
    });

    it("keeps silent playback moving through updates faster than a frame", async () => {
      let nowMs = 0;
      vi.spyOn(performance, "now").mockImplementation(() => nowMs);
      const silent = audioTimeline(300, null, 30);
      const { preview, session, worker } = await started();
      loaded(worker, 300, silent, 30);
      preview.play();

      for (let edit = 0; edit < 60; edit += 1) {
        nowMs += 16;
        session.update({ composition: composition(`edit ${edit}`) });
        loaded(worker, 300, silent, 30);
      }

      const clocks = worker.ofType("load").map((load) => load.clock);
      expect(new Set(clocks.slice(1).map((clock) => JSON.stringify(clock))).size).toBe(1);
      expect(clockPositionMs(clocks.at(-1)!, performance.timeOrigin + nowMs)).toBeCloseTo(960);
      expect(worker.ofType("seek")).toEqual([]);
      preview.pause();
      expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 960, rate: 0 });
    });

    it("keeps silent playback in the loop the worker shows", async () => {
      let nowMs = 0;
      vi.spyOn(performance, "now").mockImplementation(() => nowMs);
      const { preview, session, worker } = await playable(undefined, null);
      preview.play();

      nowMs = 14_000;
      session.update({ composition: composition("second") });
      loaded(worker, 100, null);
      expect(worker.ofType("seek")).toEqual([]);

      preview.pause();
      expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 4_000, rate: 0 });
    });

    it("applies a seek during the load to the new length, past the old end or clamped", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      preview.play();

      session.update({ composition: composition("longer") });
      void preview.seek(150);
      expect(preview.getSnapshot()).toMatchObject({ frame: 150, isPlaying: true });
      loaded(worker, 200);
      expect(preview.getSnapshot()).toMatchObject({ frame: 150, isPlaying: true });
      expect(engine?.play).toHaveBeenLastCalledWith(15_000);

      session.update({ composition: composition("colour") });
      void preview.seek(250);
      loaded(worker, 200);
      expect(preview.getSnapshot()).toMatchObject({ frame: 199, isPlaying: true });
      expect(engine?.seekMs).toHaveBeenLastCalledWith(19_900);
      expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 19_900 });
    });

    it("restarts the audio at the current position when its timeline changed", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      void preview.seek(20);
      preview.play();

      session.update({ composition: composition("second") });
      audio.positionMs = 2_500;
      loaded(worker, 100, audioTimeline(100, "voice"));

      expect(engine?.loadAudioTimeline).toHaveBeenLastCalledWith(audioTimeline(100, "voice"));
      expect(engine?.play).toHaveBeenLastCalledWith(2_500);
      expect(preview.getSnapshot()).toMatchObject({ frame: 25, isPlaying: true });
      expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 2_500 });
    });

    it("stays paused on the frame when it was paused", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      void preview.seek(30);

      session.update({ composition: composition("second") });
      expect(worker.ofType("load").at(-1)?.clock).toMatchObject({ originMs: 3_000, rate: 0 });
      loaded(worker, 100);

      expect(preview.getSnapshot()).toMatchObject({ frame: 30, isPlaying: false });
      expect(worker.ofType("seek")).toHaveLength(1);
      expect(engine?.play).not.toHaveBeenCalled();
    });

    it("parks on the last frame of a shorter composition", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      void preview.seek(80);

      session.update({ composition: composition("second") });
      loaded(worker, 50);

      expect(preview.getSnapshot()).toMatchObject({ frame: 49, isPlaying: false, totalFrames: 50 });
      expect(engine?.seekMs).toHaveBeenLastCalledWith(4_900);
      expect(worker.ofType("seek")).toHaveLength(1);
    });

    it("carries playback round a shorter loop, and restarts the audio there", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      void preview.seek(80);
      preview.play();

      session.update({ composition: composition("second") });
      loaded(worker, 50);

      expect(preview.getSnapshot()).toMatchObject({ frame: 30, isPlaying: true, totalFrames: 50 });
      expect(engine?.play).toHaveBeenLastCalledWith(3_000);
      expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 3_000 });
    });

    it("starts at the first frame for a composition shown for the first time", async () => {
      const { preview, worker } = await playable();
      expect(preview.getSnapshot()).toMatchObject({ frame: 0, isPlaying: false });
      expect(worker.ofType("seek")).toEqual([]);
    });

    it("sends a pause during the load to the worker at once", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      void preview.seek(30);
      preview.play();
      session.update({ composition: composition("second") });

      audio.positionMs = 3_500;
      preview.pause();
      expect(preview.getSnapshot().isPlaying).toBe(false);
      expect(worker.posted.at(-1)).toMatchObject({
        clock: { originMs: 3_500, rate: 0 },
        type: "clock",
      });
      loaded(worker, 100);

      expect(preview.getSnapshot()).toMatchObject({ frame: 35, isPlaying: false });
      expect(worker.ofType("seek")).toHaveLength(1);
      expect(engine?.play).toHaveBeenCalledTimes(1);
    });

    it("lets a seek during the load win over the saved frame", async () => {
      const { preview, session, worker } = await playable();
      void preview.seek(30);
      session.update({ composition: composition("second") });

      void preview.seek(80);
      loaded(worker, 100);

      expect(preview.getSnapshot()).toMatchObject({ frame: 80, isPlaying: false });
      expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 8_000 });
    });

    it("keeps a seek through updates that overlap, and ignores the replaced load", async () => {
      const { preview, session, worker } = await playable();
      preview.play();

      session.update({ composition: composition("second") });
      const stale = worker.ofType("load").at(-1)?.loadId as number;
      void preview.seek(150);
      session.update({ composition: composition("third") });
      worker.emit({
        audioTimelineJson: null,
        fps: 10,
        height: 720,
        loadId: stale,
        totalFrames: 20,
        type: "loaded",
        width: 1280,
      });
      expect(preview.getSnapshot()).toMatchObject({
        frame: 150,
        isPlaying: true,
        totalFrames: 100,
      });

      loaded(worker, 80);
      expect(preview.getSnapshot()).toMatchObject({ frame: 79, isPlaying: true, totalFrames: 80 });
      expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 7_900 });
    });

    it("stops playing on a failed load, and starts the next one where it stopped", async () => {
      const { preview, session, worker } = await playable();
      void preview.seek(30);
      preview.play();

      session.update({ composition: composition("broken") });
      worker.emit({
        diagnostics: [],
        loadId: worker.ofType("load").at(-1)?.loadId as number,
        message: "bad",
        type: "load-failed",
      });
      expect(preview.getSnapshot()).toMatchObject({ frame: 30, isLoaded: false, isPlaying: false });

      session.update({ composition: composition("fixed") });
      expect(worker.ofType("load").at(-1)?.clock).toMatchObject({ originMs: 3_000, rate: 0 });
      loaded(worker, 100);
      expect(preview.getSnapshot()).toMatchObject({ frame: 30, isPlaying: false });
    });

    it("forgets the position when the composition is removed", async () => {
      const { preview, session, worker } = await playable();
      void preview.seek(30);

      session.update({ composition: null });
      expect(audio.instances[0]?.loadAudioTimeline).toHaveBeenLastCalledWith(null);
      session.update({ composition: composition("again") });
      expect(worker.ofType("load").at(-1)?.clock).toMatchObject({ originMs: 0, rate: 0 });
      loaded(worker, 100);

      expect(preview.getSnapshot().frame).toBe(0);
    });
  });

  it("ignores events from a load that has been replaced", async () => {
    const { preview, session, worker } = await playable();
    const staleLoadId = worker.ofType("load").at(-1)?.loadId as number;
    session.update({ composition: composition("second") });

    worker.emit({ diagnostics: [], loadId: staleLoadId, message: "too late", type: "load-failed" });
    expect(preview.getSnapshot().error).toBeNull();
  });

  it("loads only for a real change, and sends only the sources that changed", async () => {
    const video = new Blob(["v"], { type: "video/mp4" });
    const logo = new Blob(["i"], { type: "image/png" });
    const nextLogo = new Blob(["j"], { type: "image/png" });
    const font = new Uint8Array([1]);
    const first = composition("first");
    const { session, worker } = await playable({
      composition: first,
      fonts: { Inter: font },
      media: { clip: video, logo },
    });
    expect(
      worker
        .ofType("load")
        .at(-1)
        ?.media.map(({ id }) => id),
    ).toEqual(["clip", "logo"]);

    session.update({
      composition: composition("first"),
      fonts: { Inter: font },
      media: { clip: { source: video }, logo },
    });
    expect(worker.ofType("load")).toHaveLength(1);

    session.update({
      composition: first,
      fonts: { Inter: font },
      media: { clip: video, logo: nextLogo },
    });
    const load = worker.ofType("load").at(-1);
    expect(worker.ofType("load")).toHaveLength(2);
    expect(load?.media.map(({ id }) => id)).toEqual(["logo"]);
    expect(load?.keepMedia).toEqual(["clip", "logo"]);
    expect(load?.fonts).toEqual([]);
    expect(load?.keepFonts).toEqual(["Inter"]);
  });

  it("sends what a model_3d reads, or a glTF blob, as a model", async () => {
    const fox = new Blob(["glTF"]);
    const ship = new Blob(["glTF"], { type: "model/gltf-binary" });
    const { worker } = await playable({
      composition: {
        nodes: [{ id: 1, type: "model_3d", params: { source: "fox" } }],
      } as unknown as LumenComposition,
      media: { fox, ship },
    });

    expect(worker.ofType("load").at(-1)?.media).toEqual([
      { id: "fox", kind: "model", source: fox },
      { id: "ship", kind: "model", source: ship },
    ]);
  });

  it("doesn't retry a failed load until an input really changes", async () => {
    const onError = vi.fn();
    const { preview, session, worker } = await started({ onError });
    const diagnostics = [
      {
        code: "node.unknown_type",
        message: "node 1: unknown or unsupported node type `blur2`",
        node: 1,
        path: "/nodes/0/type",
        severity: "error" as const,
      },
    ];
    worker.emit({
      diagnostics,
      loadId: worker.ofType("load").at(-1)?.loadId as number,
      message: "bad composition",
      type: "load-failed",
    });
    expect(preview.getSnapshot().errorCode).toBe("load_failed");
    expect(onError).toHaveBeenCalledExactlyOnceWith({
      code: "load_failed",
      diagnostics,
      message: "bad composition",
    });

    session.update({ composition: composition("first") });
    expect(worker.ofType("load")).toHaveLength(1);

    session.update({ composition: composition("second") });
    expect(worker.ofType("load")).toHaveLength(2);
  });

  it("reports inputs that don't resolve once, until they change", async () => {
    const onError = vi.fn();
    const { session } = await started({ onError });
    const unresolved = { composition: composition("first"), media: { unused: "/image.png" } };
    session.update({ ...unresolved, onError });
    expect(onError).toHaveBeenCalledOnce();
    expect(onError.mock.calls[0]?.[0]).toMatchObject({ code: "load_failed", diagnostics: [] });

    // A callback that re-renders with the same inputs doesn't hear the same failure again.
    const replaced = vi.fn();
    session.update({ ...unresolved, onError: replaced });
    session.update({ ...unresolved, media: { unused: "/image.png" }, onError: replaced });
    expect(onError).toHaveBeenCalledOnce();
    expect(replaced).not.toHaveBeenCalled();

    session.update({ composition: composition("first"), onError: replaced });
    session.update({ ...unresolved, onError: replaced });
    expect(replaced).toHaveBeenCalledOnce();
  });

  it("keeps a resolution error over an earlier load, then clears it when valid inputs load", async () => {
    const { preview, session, worker } = await started({});
    session.update({ composition: composition("first") });
    const first = worker.ofType("load").at(-1)?.loadId as number;
    const unresolved = { composition: composition("second"), media: { unused: "/image.png" } };
    session.update(unresolved);
    expect(preview.getSnapshot().errorCode).toBe("load_failed");

    worker.emit({
      audioTimelineJson: null,
      fps: 30,
      height: 16,
      loadId: first,
      totalFrames: 30,
      type: "loaded",
      width: 16,
    });
    expect(preview.getSnapshot().errorCode).toBe("load_failed");
    session.update(unresolved);
    expect(preview.getSnapshot().errorCode).toBe("load_failed");

    session.update({ composition: composition("first") });
    const retry = worker.ofType("load").at(-1)?.loadId as number;
    expect(retry).toBeGreaterThan(first);
    worker.emit({
      audioTimelineJson: null,
      fps: 30,
      height: 16,
      loadId: retry,
      totalFrames: 30,
      type: "loaded",
      width: 16,
    });
    expect(preview.getSnapshot().errorCode).toBeNull();
  });

  it("sends a source again if the worker never received it", async () => {
    const { session, worker } = await playable();
    const logo = new Blob(["i"], { type: "image/png" });
    const postMessage = worker.postMessage;
    worker.postMessage = () => {
      throw new DOMException("could not be cloned", "DataCloneError");
    };
    session.update({ composition: composition("first"), media: { logo } });
    worker.postMessage = postMessage;

    session.update({ composition: composition("second"), media: { logo } });
    expect(
      worker
        .ofType("load")
        .at(-1)
        ?.media.map(({ id }) => id),
    ).toEqual(["logo"]);
  });

  it("resolves a seek when its frame is shown, and false when a load replaces it", async () => {
    const { preview, session, worker } = await playable();
    const shown = preview.seek(30);
    const seekId = worker.ofType("seek").at(-1)?.seekId as number;
    worker.emit({
      actualFps: 0,
      frame: 30,
      loadId: worker.ofType("load").at(-1)?.loadId as number,
      renderMs: 1,
      seekId,
      type: "frame",
    });
    await expect(shown).resolves.toBe(true);

    const replaced = preview.seek(40);
    session.update({ composition: composition("second") });
    await expect(replaced).resolves.toBe(false);
  });

  it("keeps following frames after a seek and a new attach", async () => {
    const { preview, session, worker } = await playable();
    void preview.seek(30);
    expect(worker.ofType("seek").at(-1)?.seekId).toBe(1);

    const attached = session.attach({ append: () => {} } as unknown as HTMLElement);
    expect(preview.getSnapshot().isLoaded).toBe(false);
    const nextWorker = FakeWorker.instances.at(-1) as FakeWorker;
    nextWorker.emit({ type: "ready" });
    await attached;
    const loadId = nextWorker.ofType("load").at(-1)?.loadId as number;
    nextWorker.emit({
      audioTimelineJson: null,
      fps: 10,
      height: 720,
      loadId,
      totalFrames: 100,
      type: "loaded",
      width: 1280,
    });

    nextWorker.emit({ actualFps: 10, frame: 5, loadId, renderMs: 1, seekId: 0, type: "frame" });
    expect(preview.getSnapshot().frame).toBe(5);
  });

  it("makes a render failure stop playback, and clears it when a frame arrives", async () => {
    const { preview, worker } = await playable();
    const loadId = worker.ofType("load").at(-1)?.loadId as number;
    preview.play();
    worker.emit({ loadId, message: "device lost", type: "render-failed" });
    expect(preview.getSnapshot()).toMatchObject({
      error: "device lost",
      errorCode: "render_failed",
      isPlaying: false,
    });

    worker.emit({ actualFps: 0, frame: 1, loadId, renderMs: 1, seekId: 0, type: "frame" });
    expect(preview.getSnapshot()).toMatchObject({ error: null, errorCode: null });
  });
});
