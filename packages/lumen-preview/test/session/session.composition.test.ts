import { describe, expect, it, vi } from "vitest";

import { clockPositionMs } from "../../src/clock.js";
import { FakeWorker } from "../support/fake-worker.js";
import { audio } from "./fake-audio-engine.js";
import { audioTimeline, composition, playable, started, useSessionEnvironment } from "./support.js";

vi.mock("../../src/audio-engine.js", async () => ({
  LumenAudioEngine: (await import("./fake-audio-engine.js")).FakeAudioEngine,
}));

describe("LumenPreviewSession: composition", () => {
  useSessionEnvironment();

  describe("updating the composition", () => {
    function loaded(
      worker: FakeWorker,
      totalFrames: number,
      audioTimelineJson: string | null = audioTimeline(),
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

    it("reads a composition object again on every call, so one edited in place loads", async () => {
      let document = composition("A");
      const built = { toJSON: () => document };
      const { session, worker } = await started({ composition: built });
      session.setInputs({ composition: built });
      expect(worker.ofType("load")).toHaveLength(1);

      document = composition("B");
      session.setInputs({ composition: built });

      expect(worker.ofType("load")).toHaveLength(2);
      expect(JSON.parse(worker.ofType("load")[1]?.compositionJson as string)).toEqual(document);
    });

    it("keeps playing through the load, and the audio isn't touched when its timeline is the same", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      void preview.seek(30);
      preview.play();
      const calls = (engine?.play.mock.calls.length ?? 0) + (engine?.seekMs.mock.calls.length ?? 0);

      session.setInputs({ composition: composition("second") });
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
      loaded(worker, 300, audioTimeline("music"), 30);
      const loadId = worker.ofType("load").at(-1)?.loadId as number;
      preview.play();
      audio.clockRunning = true;
      audio.positionMs = 1_060;
      worker.emit({ actualFps: 30, frame: 32, loadId, renderMs: 1, seekId: 0, type: "frame" });

      session.setInputs({ composition: composition("colour") });
      loaded(worker, 300, audioTimeline("music"), 30);

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
      const silent = audioTimeline(null);
      const { preview, session, worker } = await started();
      loaded(worker, 300, silent, 30);
      preview.play();

      for (let edit = 0; edit < 60; edit += 1) {
        nowMs += 16;
        session.setInputs({ composition: composition(`edit ${edit}`) });
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
      const { preview, session, worker } = await playable({ loop: true }, null);
      preview.play();

      nowMs = 14_000;
      session.setInputs({ composition: composition("second"), loop: true });
      loaded(worker, 100, null);
      expect(worker.ofType("seek")).toEqual([]);

      preview.pause();
      expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 4_000, rate: 0 });
    });

    it("applies a seek during the load to the new length, past the old end or clamped", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      preview.play();

      session.setInputs({ composition: composition("longer") });
      void preview.seek(150);
      expect(preview.getSnapshot()).toMatchObject({ frame: 150, isPlaying: true });
      loaded(worker, 200);
      expect(preview.getSnapshot()).toMatchObject({ frame: 150, isPlaying: true });
      expect(engine?.play).toHaveBeenLastCalledWith(15_000);

      session.setInputs({ composition: composition("colour") });
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

      session.setInputs({ composition: composition("second") });
      audio.positionMs = 2_500;
      loaded(worker, 100, audioTimeline("voice"));

      expect(engine?.loadAudioTimeline).toHaveBeenLastCalledWith(audioTimeline("voice"), {
        durationFrames: 100,
        fps: 10,
      });
      expect(engine?.play).toHaveBeenLastCalledWith(2_500);
      expect(preview.getSnapshot()).toMatchObject({ frame: 25, isPlaying: true });
      expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 2_500 });
    });

    it("stays paused on the frame when it was paused", async () => {
      const { preview, session, worker } = await playable();
      const engine = audio.instances[0];
      void preview.seek(30);

      session.setInputs({ composition: composition("second") });
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

      session.setInputs({ composition: composition("second"), loop: true });
      loaded(worker, 50);

      expect(preview.getSnapshot()).toMatchObject({ frame: 49, isPlaying: false, totalFrames: 50 });
      expect(engine?.seekMs).toHaveBeenLastCalledWith(4_900);
      expect(worker.ofType("seek")).toHaveLength(1);
    });

    it("carries playback round a shorter loop, and restarts the audio there", async () => {
      const { preview, session, worker } = await playable({ loop: true });
      const engine = audio.instances[0];
      void preview.seek(80);
      preview.play();

      session.setInputs({ composition: composition("second"), loop: true });
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
      session.setInputs({ composition: composition("second") });

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
      session.setInputs({ composition: composition("second") });

      void preview.seek(80);
      loaded(worker, 100);

      expect(preview.getSnapshot()).toMatchObject({ frame: 80, isPlaying: false });
      expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 8_000 });
    });

    it("keeps a seek through updates that overlap, and ignores the replaced load", async () => {
      const { preview, session, worker } = await playable();
      preview.play();

      session.setInputs({ composition: composition("second") });
      const stale = worker.ofType("load").at(-1)?.loadId as number;
      void preview.seek(150);
      session.setInputs({ composition: composition("third") });
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

      session.setInputs({ composition: composition("broken") });
      worker.emit({
        diagnostics: [],
        loadId: worker.ofType("load").at(-1)?.loadId as number,
        media: null,
        message: "bad",
        type: "load-failed",
      });
      expect(preview.getSnapshot()).toMatchObject({ frame: 30, isLoaded: false, isPlaying: false });

      session.setInputs({ composition: composition("fixed") });
      expect(worker.ofType("load").at(-1)?.clock).toMatchObject({ originMs: 3_000, rate: 0 });
      loaded(worker, 100);
      expect(preview.getSnapshot()).toMatchObject({ frame: 30, isPlaying: false });
    });

    it("forgets the position when the composition is removed", async () => {
      const { preview, session, worker } = await playable();
      void preview.seek(30);

      session.setInputs({ composition: null });
      expect(audio.instances[0]?.loadAudioTimeline).toHaveBeenLastCalledWith(null);
      session.setInputs({ composition: composition("again") });
      expect(worker.ofType("load").at(-1)?.clock).toMatchObject({ originMs: 0, rate: 0 });
      loaded(worker, 100);

      expect(preview.getSnapshot().frame).toBe(0);
    });
  });
});
