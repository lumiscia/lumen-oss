import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LumenComposition } from "@lumiscia/lumen-types";

import { LumenPreviewContext } from "../../src/preview.js";
import { LumenPreviewSession } from "../../src/session/session.js";
import type { LumenPreviewWorkerCommand, LumenPreviewWorkerEvent } from "../../src/worker-host.js";

const audio = vi.hoisted(() => ({
  clockRunning: false,
  instances: [] as {
    loadAudioTimeline: ReturnType<typeof vi.fn>;
    options: { onClockChange: () => void };
    play: ReturnType<typeof vi.fn>;
  }[],
  playing: false,
  positionMs: 0,
}));

vi.mock("../../src/audio-engine.js", () => ({
  LumenAudioEngine: class {
    loadAudioTimeline = vi.fn(() => {
      audio.playing = false;
    });
    play = vi.fn(() => {
      audio.playing = true;
    });
    constructor(readonly options: { onClockChange: () => void }) {
      audio.instances.push(this);
    }
    currentTimeMs = () => audio.positionMs;
    dispose() {}
    isClockRunning = () => audio.clockRunning;
    isPlaying = () => audio.playing;
    pause() {
      audio.playing = false;
    }
    seekMs(ms: number) {
      audio.positionMs = ms;
    }
    syncAudioSources = async () => {};
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

function composition(name: string): LumenComposition {
  return { metadata: { name } } as unknown as LumenComposition;
}

function createSession(inputs: ConstructorParameters<typeof LumenPreviewSession>[0] | object = {}) {
  const preview = new LumenPreviewContext();
  const canvases: unknown[] = [];
  const host = { append: (canvas: unknown) => canvases.push(canvas) } as unknown as HTMLElement;
  const session = new LumenPreviewSession({
    bindings: {
      preview: async () => {
        throw new Error("a session loads its bindings in the worker");
      },
      previewWasmUrl: () => "https://a.test/lumen.wasm",
      previewWorkerUrl: () => "https://a.test/worker.js",
    },
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

async function playable(inputs?: object) {
  const fake = await started(inputs);
  const { loadId } = fake.worker.ofType("load").at(-1) as { loadId: number };
  fake.worker.emit({
    audioTimelineJson: null,
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
    vi.stubGlobal("document", { createElement: () => new FakeCanvas() });
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
    expect(canvases).toHaveLength(0);
    expect(worker.terminated).toBe(true);
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
    const session = new LumenPreviewSession({ bindings: {} as never, preview });
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

  it("stops at once for a new composition and stays stopped once it's loaded", async () => {
    const { preview, session, worker } = await playable();
    const engine = audio.instances[0];
    preview.play();
    expect(engine?.play).toHaveBeenCalledOnce();

    session.update({ composition: composition("second") });
    expect(preview.getSnapshot().isPlaying).toBe(false);
    expect(engine?.loadAudioTimeline).toHaveBeenLastCalledWith(null);

    worker.emit({
      audioTimelineJson: '{"clips":[]}',
      fps: 10,
      height: 720,
      loadId: worker.ofType("load").at(-1)?.loadId as number,
      totalFrames: 50,
      type: "loaded",
      width: 1280,
    });
    expect(engine?.loadAudioTimeline).toHaveBeenLastCalledWith('{"clips":[]}');
    expect(engine?.play).toHaveBeenCalledOnce();
    expect(preview.getSnapshot()).toMatchObject({ isPlaying: false, totalFrames: 50 });
  });

  it("ignores events from a load that has been replaced", async () => {
    const { preview, session, worker } = await playable();
    const staleLoadId = worker.ofType("load").at(-1)?.loadId as number;
    session.update({ composition: composition("second") });

    worker.emit({ loadId: staleLoadId, message: "too late", type: "load-failed" });
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

  it("doesn't retry a failed load until an input really changes", async () => {
    const { preview, session, worker } = await started();
    worker.emit({
      loadId: worker.ofType("load").at(-1)?.loadId as number,
      message: "bad composition",
      type: "load-failed",
    });
    expect(preview.getSnapshot().errorCode).toBe("load_failed");

    session.update({ composition: composition("first") });
    expect(worker.ofType("load")).toHaveLength(1);

    session.update({ composition: composition("second") });
    expect(worker.ofType("load")).toHaveLength(2);
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
