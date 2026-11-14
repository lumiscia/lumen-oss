import { describe, expect, it, vi } from "vitest";

import type { LumenPreviewWorkerEvent } from "../../src/worker-host.js";
import { FakeWorker } from "../support/fake-worker.js";
import { audio } from "./fake-audio-engine.js";
import { composition, playable, started, unreadable, useSessionEnvironment } from "./support.js";

vi.mock("../../src/audio-engine.js", async () => ({
  LumenAudioEngine: (await import("./fake-audio-engine.js")).FakeAudioEngine,
}));

describe("LumenPreviewSession: loading", () => {
  useSessionEnvironment();

  it("ignores events from a load that has been replaced", async () => {
    const { preview, session, worker } = await playable();
    const staleLoadId = worker.ofType("load").at(-1)?.loadId as number;
    session.setInputs({ composition: composition("second") });

    worker.emit({
      diagnostics: [],
      loadId: staleLoadId,
      media: null,
      message: "too late",
      type: "load-failed",
    });
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
        ?.media.map(([source]) => source),
    ).toEqual(["clip", "logo"]);

    session.setInputs({
      composition: composition("first"),
      fonts: { Inter: font },
      media: { clip: { source: video }, logo },
    });
    expect(worker.ofType("load")).toHaveLength(1);

    session.setInputs({
      composition: first,
      fonts: { Inter: font },
      media: { clip: video, logo: nextLogo },
    });
    const load = worker.ofType("load").at(-1);
    expect(worker.ofType("load")).toHaveLength(2);
    expect(load?.media.map(([source]) => source)).toEqual(["logo"]);
    expect(load?.keepMedia).toEqual(["clip", "logo"]);
    expect(load?.fonts).toEqual([]);
    expect(load?.keepFonts).toEqual(["Inter"]);
  });

  it("sends each media entry as given, with its URL made absolute, for the worker to match", async () => {
    const fox = new Blob(["glTF"]);
    const { worker } = await playable({
      media: { clip: { fps: 24, source: "clip.mp4" }, fox },
    });

    expect(worker.ofType("load").at(-1)?.media).toEqual([
      ["clip", { fps: 24, source: "https://a.test/clip.mp4" }],
      ["fox", { source: fox }],
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
      media: null,
      message: "bad composition",
      type: "load-failed",
    });
    expect(preview.getSnapshot().errorCode).toBe("load_failed");
    expect(onError).toHaveBeenCalledExactlyOnceWith({
      code: "load_failed",
      diagnostics,
      media: null,
      message: "bad composition",
    });

    session.setInputs({ composition: composition("first") });
    expect(worker.ofType("load")).toHaveLength(1);

    session.setInputs({ composition: composition("second") });
    expect(worker.ofType("load")).toHaveLength(2);
  });

  it("reports the engine's code for a frame that failed to render", async () => {
    const onError = vi.fn();
    const { worker } = await playable({ onError });
    const loadId = worker.ofType("load").at(-1)?.loadId as number;
    const diagnostics = [
      { code: "render.limit", message: "over the limit", path: "", severity: "error" as const },
    ];

    worker.emit({
      diagnostics,
      loadId,
      media: null,
      message: "over the limit",
      type: "render-failed",
    });

    expect(onError).toHaveBeenLastCalledWith({
      code: "render_failed",
      diagnostics,
      media: null,
      message: "over the limit",
    });
  });

  it("reports the media source a load or a frame couldn't use", async () => {
    const onError = vi.fn();
    const { worker } = await playable({ onError });
    const loadId = worker.ofType("load").at(-1)?.loadId as number;
    const media = {
      code: "media.wrong_kind" as const,
      source: "asset:logo",
      wanted: ["model" as const],
      found: "image" as const,
      at: { type: "node" as const, node: 3, param: "source" },
      details: "",
    };

    worker.emit({
      diagnostics: [],
      loadId,
      media,
      message: "`asset:logo` is an image, not a model",
      type: "render-failed",
    });

    expect(onError).toHaveBeenLastCalledWith({
      code: "render_failed",
      diagnostics: [],
      media,
      message: "`asset:logo` is an image, not a model",
    });
  });

  it("reports inputs that don't resolve once, until they change", async () => {
    const onError = vi.fn();
    const { session } = await started({ onError });
    const unresolved = { composition: unreadable() };
    session.setInputs({ ...unresolved, onError });
    expect(onError).toHaveBeenCalledOnce();
    expect(onError.mock.calls[0]?.[0]).toMatchObject({ code: "load_failed", diagnostics: [] });

    // A callback that re-renders with the same inputs doesn't hear the same failure again.
    const replaced = vi.fn();
    session.setInputs({ ...unresolved, onError: replaced });
    session.setInputs({ composition: unreadable(), onError: replaced });
    expect(onError).toHaveBeenCalledOnce();
    expect(replaced).not.toHaveBeenCalled();

    session.setInputs({ composition: composition("first"), onError: replaced });
    session.setInputs({ ...unresolved, onError: replaced });
    expect(replaced).toHaveBeenCalledOnce();
  });

  it("keeps a resolution error over an earlier load, then clears it when valid inputs load", async () => {
    const { preview, session, worker } = await started({});
    session.setInputs({ composition: composition("first") });
    const first = worker.ofType("load").at(-1)?.loadId as number;
    const unresolved = { composition: unreadable() };
    session.setInputs(unresolved);
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
    session.setInputs(unresolved);
    expect(preview.getSnapshot().errorCode).toBe("load_failed");

    session.setInputs({ composition: composition("first") });
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
    session.setInputs({ composition: composition("first"), media: { logo } });
    worker.postMessage = postMessage;

    session.setInputs({ composition: composition("second"), media: { logo } });
    expect(
      worker
        .ofType("load")
        .at(-1)
        ?.media.map(([source]) => source),
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
    session.setInputs({ composition: composition("second") });
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
    worker.emit({
      diagnostics: [],
      loadId,
      media: null,
      message: "device lost",
      type: "render-failed",
    });
    expect(preview.getSnapshot()).toMatchObject({
      error: "device lost",
      errorCode: "render_failed",
      isPlaying: false,
    });

    worker.emit({ actualFps: 0, frame: 1, loadId, renderMs: 1, seekId: 0, type: "frame" });
    expect(preview.getSnapshot()).toMatchObject({ error: null, errorCode: null });
  });

  describe("media loading", () => {
    type Entry = Extract<LumenPreviewWorkerEvent, { type: "media" }>["media"][number];
    const entry = (source: string, kind: Entry["kind"], status: Entry["status"]): Entry => ({
      failure: null,
      kind,
      source,
      status,
    });
    const currentLoad = (worker: FakeWorker) => worker.ofType("load").at(-1)?.loadId as number;
    const loaded = (worker: FakeWorker) =>
      worker.emit({
        audioTimelineJson: null,
        fps: 10,
        height: 720,
        loadId: currentLoad(worker),
        totalFrames: 100,
        type: "loaded",
        width: 1280,
      });
    const media = (worker: FakeWorker, entries: Entry[]) =>
      worker.emit({ loadId: currentLoad(worker), media: entries, type: "media" });
    const shown = (worker: FakeWorker) =>
      worker.emit({
        actualFps: 0,
        frame: 0,
        loadId: currentLoad(worker),
        renderMs: 1,
        seekId: 0,
        type: "frame",
      });

    it("counts what the composition needs in the state, and is ready once all of it is", async () => {
      const { preview, session, worker } = await started();
      let ready: unknown = "waiting";
      void preview.whenReady().then((error) => (ready = error));

      media(worker, [entry("logo", "image", "pending"), entry("song", "audio", "pending")]);
      expect(preview.getSnapshot().media).toEqual({ failed: 0, pending: 2, ready: 0 });
      media(worker, [entry("logo", "image", "ready"), entry("song", "audio", "pending")]);
      loaded(worker);
      shown(worker);
      await Promise.resolve();
      expect(ready).toBe("waiting");

      media(worker, [entry("logo", "image", "ready"), entry("song", "audio", "ready")]);
      await Promise.resolve();
      expect(ready).toBeNull();
      expect(preview.getSnapshot().media).toEqual({ failed: 0, pending: 0, ready: 2 });
      await expect(session.whenReady()).resolves.toBeNull();
    });

    it("waits for the newest composition, and reports why one failed", async () => {
      const { session, worker } = await started();
      const waiting = session.whenReady();
      session.setInputs({ composition: composition("second") });
      const failure = {
        code: "media.not_found" as const,
        details: "`media` has no entry by this name",
        source: "sky.hdr",
        wanted: ["environment" as const],
      };
      worker.emit({
        diagnostics: [],
        loadId: currentLoad(worker),
        media: failure,
        message: "sky.hdr: `media` has no entry by this name",
        type: "load-failed",
      });

      await expect(waiting).resolves.toMatchObject({ code: "load_failed", media: failure });
      session.setInputs({ composition: null });
      await expect(session.whenReady()).resolves.toBeNull();
    });

    it("is ready with an audio failure, which plays silent", async () => {
      const { session, worker } = await started();
      const failure = {
        code: "media.decode" as const,
        details: "corrupt",
        source: "song",
        wanted: ["audio" as const],
      };
      loaded(worker);
      shown(worker);
      media(worker, [{ ...entry("song", "audio", "failed"), failure }]);

      await expect(session.whenReady()).resolves.toEqual({
        code: "audio_failed",
        diagnostics: [],
        media: failure,
        message: "song: corrupt",
      });
    });

    it("isn't ready until a frame is drawn, and is ready with the error of a first render that fails", async () => {
      const { session, worker } = await started();
      let ready: unknown = "waiting";
      void session.whenReady().then((error) => (ready = error));
      loaded(worker);
      media(worker, [entry("fox", "model", "ready")]);
      await Promise.resolve();
      expect(ready).toBe("waiting");

      const failure = {
        code: "media.decode" as const,
        details: "not a model",
        source: "fox",
        wanted: ["model" as const],
      };
      worker.emit({
        diagnostics: [],
        loadId: currentLoad(worker),
        media: failure,
        message: "fox: not a model",
        type: "render-failed",
      });
      await Promise.resolve();
      expect(ready).toMatchObject({ code: "render_failed", media: failure });
    });

    it("decodes the audio the worker asks for, answers it, and drops it when released", async () => {
      const { worker } = await started();
      const engine = audio.instances.at(-1);
      engine?.loadSource.mockImplementation(async (id: string) => {
        if (id === "bad") {
          throw Object.assign(new Error("corrupt"), {
            media: { code: "media.decode", details: "corrupt", source: "bad", wanted: ["audio"] },
          });
        }
      });

      worker.emit({
        input: "https://a.test/song.wav",
        requestId: 1,
        source: "song",
        type: "audio-source",
      });
      worker.emit({
        input: "https://a.test/bad.wav",
        requestId: 2,
        source: "bad",
        type: "audio-source",
      });
      worker.emit({ source: "song", type: "audio-source-released" });
      await new Promise((resolve) => setTimeout(resolve));

      expect(engine?.loadSource).toHaveBeenCalledWith("song", "https://a.test/song.wav");
      expect(engine?.removeSource).toHaveBeenCalledWith("song");
      expect(worker.ofType("audio-source-settled")).toEqual([
        { failure: null, requestId: 1, type: "audio-source-settled" },
        {
          failure: { code: "media.decode", details: "corrupt", source: "bad", wanted: ["audio"] },
          requestId: 2,
          type: "audio-source-settled",
        },
      ]);
    });

    it("holds playback at its position while media is pending, then plays on from there", async () => {
      const { preview, worker } = await playable();
      const engine = audio.instances.at(-1);
      media(worker, [entry("song", "audio", "pending")]);

      preview.play();
      expect(preview.getSnapshot().isPlaying).toBe(true);
      expect(engine?.play).not.toHaveBeenCalled();
      expect(worker.ofType("clock").at(-1)?.clock.rate ?? 0).toBe(0);

      media(worker, [entry("song", "audio", "ready")]);
      expect(engine?.play).toHaveBeenCalledOnce();

      // A composition change that needs more media parks playing again, where it is.
      media(worker, [entry("song", "audio", "ready"), entry("clip", "video", "pending")]);
      expect(engine?.pause).toHaveBeenCalled();
      expect(worker.ofType("clock").at(-1)?.clock.rate).toBe(0);
      expect(preview.getSnapshot().isPlaying).toBe(true);
    });

    it("holds playback while the worker waits for the frame it is on", async () => {
      const { preview, worker } = await playable();
      const engine = audio.instances.at(-1);
      media(worker, [entry("clip", "video", "ready")]);
      preview.play();
      expect(engine?.play).toHaveBeenCalledOnce();

      worker.emit({ loadId: currentLoad(worker), type: "waiting", waiting: true });
      expect(engine?.pause).toHaveBeenCalled();
      expect(worker.ofType("clock").at(-1)?.clock.rate).toBe(0);

      worker.emit({ loadId: currentLoad(worker), type: "waiting", waiting: false });
      expect(engine?.play).toHaveBeenCalledTimes(2);
    });
  });
});
