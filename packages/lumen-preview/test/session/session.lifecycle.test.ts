import { describe, expect, it, vi } from "vitest";

import { LumenPreviewContext } from "../../src/preview.js";
import { LumenPreviewSession } from "../../src/session/session.js";
import { FakeWorker } from "../support/fake-worker.js";
import { audio } from "./fake-audio-engine.js";
import {
  composition,
  createSession,
  FakeCanvas,
  playable,
  RUNTIME,
  started,
  useSessionEnvironment,
} from "./support.js";

vi.mock("../../src/audio-engine.js", async () => ({
  LumenAudioEngine: (await import("./fake-audio-engine.js")).FakeAudioEngine,
}));

describe("LumenPreviewSession: lifecycle", () => {
  useSessionEnvironment();

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
    expect(preview.getSnapshot().error).toContain("pass `runtimeUrl`");
    expect(canvases).toHaveLength(0);
    expect(worker.terminated).toBe(true);
  });

  it("names the Vite setting when a pre-bundled worker URL doesn't load", async () => {
    const url = "https://a.test/node_modules/.vite/deps/dist/browser/worker.js";
    const { attached, preview, worker } = createSession({
      runtime: { ...RUNTIME, worker: { file: "worker.js", url } },
    });
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

    session.setInputs({ composition: composition("second"), logLevel: "warn", lookaheadCount: 2 });
    expect(worker.ofType("set-options")).toHaveLength(0);
    worker.emit({ type: "ready" });
    await attached;

    expect(worker.ofType("set-options")).toMatchObject([{ logLevel: "warn", lookaheadCount: 2 }]);
    expect(worker.ofType("load")).toHaveLength(1);
    expect(worker.ofType("load")[0]?.compositionJson).toContain("second");
    const types = worker.posted.map((message) => message.type);
    expect(types.indexOf("set-options")).toBeLessThan(types.indexOf("load"));
  });

  it("rejects an invalid update made while the worker was starting, as a whole", async () => {
    const onError = vi.fn();
    const replacementError = vi.fn();
    const { attached, session, worker } = createSession({ lookaheadCount: 8, onError });

    session.setInputs({
      composition: composition("replacement"),
      lookaheadCount: 2,
      onError: replacementError,
      rate: 9,
    });
    worker.emit({ type: "ready" });
    await attached;

    expect(onError).toHaveBeenCalledTimes(1);
    expect(replacementError).not.toHaveBeenCalled();
    expect(worker.ofType("set-options")).toEqual([]);
    expect(worker.ofType("load")).toHaveLength(1);
    expect(worker.ofType("load")[0]?.compositionJson).toContain("first");
    expect(session.rate).toBe(1);
  });

  it("reports invalid options a session starts with, and plays with the defaults", async () => {
    const onError = vi.fn();
    const { preview, session, worker } = await started({
      lookaheadCount: 2,
      onError,
      rate: 9,
      volume: 0.5,
    });

    expect(onError).toHaveBeenCalledWith(expect.objectContaining({ code: "invalid_option" }));
    expect(preview.getSnapshot().errorCode).toBe("invalid_option");
    expect([session.rate, session.volume]).toEqual([1, 1]);
    expect(worker.ofType("start")[0]).toMatchObject({ lookaheadCount: 2 });
    expect(worker.ofType("load")).toHaveLength(1);

    session.setInputs({ composition: composition("first"), rate: 2 });
    expect(preview.getSnapshot().errorCode).toBeNull();
    expect(session.rate).toBe(2);
  });

  it("sends options only when they really change", async () => {
    const { session, worker } = await started({ lookaheadCount: 8 });
    session.setInputs({ composition: composition("first") });
    expect(worker.ofType("set-options")).toHaveLength(0);

    session.setInputs({ composition: composition("first"), lookaheadCount: 2 });
    expect(worker.ofType("set-options")).toMatchObject([{ lookaheadCount: 2 }]);
  });

  it("reports a browser without WebGPU before it creates a worker", async () => {
    vi.stubGlobal("navigator", {});
    const preview = new LumenPreviewContext();
    const session = new LumenPreviewSession({ preview, runtime: RUNTIME });
    await session.attach({} as HTMLElement);

    expect(preview.getSnapshot().errorCode).toBe("webgpu_unavailable");
    expect(FakeWorker.instances).toHaveLength(0);
  });

  it("keeps the same snapshot until the state changes", async () => {
    const preview = new LumenPreviewContext();
    const first = preview.getSnapshot();
    expect(preview.getSnapshot()).toBe(first);

    const listener = vi.fn();
    preview.subscribe(listener);
    await preview.seek(5);
    const second = preview.getSnapshot();
    expect(second).not.toBe(first);
    expect(second.frame).toBe(5);
    expect(first.frame).toBe(0);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(preview.getSnapshot()).toBe(second);
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

  it("loads each runtime file from the manifest's URL", async () => {
    const { worker } = await started();

    expect(worker.url).toBe("https://a.test/worker.js");
    expect(worker.ofType("start")[0]?.wasmUrl).toBe("https://a.test/lumen.wasm");
    expect(audio.instances.at(-1)?.options).toMatchObject({
      workerUrl: "https://a.test/audio-worker.js",
      workletUrl: "https://a.test/audio-worklet.js",
    });
  });

  it("finds every runtime file in runtimeUrl under its manifest name", async () => {
    const { worker } = await started({ runtimeUrl: "/lumen" });

    expect(worker.url).toBe("https://a.test/lumen/worker-2d.js");
    expect(worker.ofType("start")[0]?.wasmUrl).toBe("https://a.test/lumen/lumen_wasm_2d_bg.wasm");
    expect(audio.instances.at(-1)?.options).toMatchObject({
      workerUrl: "https://a.test/lumen/audio-worker.js",
      workletUrl: "https://a.test/lumen/audio-worklet.js",
    });
  });

  it("names the manifest when a JavaScript caller leaves it out", () => {
    const options = { preview: new LumenPreviewContext() };
    expect(() => new LumenPreviewSession(options as never)).toThrow(/needs `runtime`/);
  });

  it("keeps the query of runtimeUrl on each file, in the directory its path names", async () => {
    const { worker } = await started({
      runtimeUrl: "https://a.test/runtime?v=1#top",
    });

    expect(worker.url).toBe("https://a.test/runtime/worker-2d.js?v=1");
    expect(worker.ofType("start")[0]?.wasmUrl).toBe(
      "https://a.test/runtime/lumen_wasm_2d_bg.wasm?v=1",
    );
  });
});
