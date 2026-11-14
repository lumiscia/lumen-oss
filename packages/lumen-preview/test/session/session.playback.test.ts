import { describe, expect, it, vi } from "vitest";

import { clockPositionMs, type PreviewClock } from "../../src/clock.js";
import { LumenPreviewSession } from "../../src/session/session.js";
import { FakeWorker } from "../support/fake-worker.js";
import { audio } from "./fake-audio-engine.js";
import { audioTimeline, composition, playable, useSessionEnvironment } from "./support.js";

vi.mock("../../src/audio-engine.js", async () => ({
  LumenAudioEngine: (await import("./fake-audio-engine.js")).FakeAudioEngine,
}));

describe("LumenPreviewSession: playback", () => {
  useSessionEnvironment();

  describe("playback options", () => {
    type Inputs = Parameters<LumenPreviewSession["setInputs"]>[0];
    const update = (session: LumenPreviewSession, inputs: Inputs = {}) =>
      session.setInputs({ composition: composition("first"), ...inputs });
    const showFrame = (worker: FakeWorker, frame: number, seekId = 0) =>
      worker.emit({
        actualFps: 10,
        frame,
        loadId: worker.ofType("load").at(-1)?.loadId as number,
        renderMs: 1,
        seekId,
        type: "frame",
      });

    // The newest playback revision the session has told the worker about.
    const revisionOf = (worker: FakeWorker) =>
      worker.posted.reduce(
        (newest, message) =>
          message.type === "clock" || message.type === "set-playback"
            ? Math.max(newest, message.revision)
            : newest,
        0,
      );
    const ended = (worker: FakeWorker, seekId = 0, playbackRevision = revisionOf(worker)) =>
      worker.emit({
        loadId: worker.ofType("load").at(-1)?.loadId as number,
        playbackRevision,
        seekId,
        type: "ended",
      });

    it("changes nothing for a player that sets none", async () => {
      const { session, worker } = await playable();
      const [engine] = audio.instances;

      expect([session.loop, session.rate, session.volume, session.muted, session.range]).toEqual([
        false,
        1,
        1,
        false,
        null,
      ]);
      expect(worker.ofType("set-playback")).toEqual([]);
      expect(engine?.setVolume).not.toHaveBeenCalled();
      expect(engine?.setRate).not.toHaveBeenCalled();
    });

    it("applies the options a session starts with, before the first load", async () => {
      const { session, worker } = await playable({
        loop: true,
        muted: true,
        range: { in: 20, out: 50 },
        rate: 2,
        volume: 0.4,
      });
      const [engine] = audio.instances;

      expect(worker.ofType("set-playback")).toMatchObject([
        { loop: true, range: { in: 20, out: 50 }, revision: 1, type: "set-playback" },
      ]);
      expect(worker.posted.findIndex(({ type }) => type === "set-playback")).toBeLessThan(
        worker.posted.findIndex(({ type }) => type === "load"),
      );
      expect(engine?.setVolume).toHaveBeenCalledWith(0.4, true);
      expect(engine?.setRate).toHaveBeenCalledWith(2);
      expect([session.loop, session.rate, session.volume, session.muted, session.range]).toEqual([
        true,
        2,
        0.4,
        true,
        { in: 20, out: 50 },
      ]);
    });

    it("updates loop and range without loading again, and only when they change", async () => {
      const { session, worker } = await playable();

      update(session, { loop: true });
      update(session, { loop: true });
      expect(worker.ofType("set-playback")).toMatchObject([
        { loop: true, range: null, revision: 1, type: "set-playback" },
      ]);
      update(session, { loop: true, range: { in: 10, out: 20 } });
      update(session, { loop: true, range: { in: 10, out: 20 } });
      expect(worker.ofType("set-playback")).toHaveLength(2);
      expect(worker.ofType("load")).toHaveLength(1);
      expect(session.range).toEqual({ in: 10, out: 20 });
    });

    it("sets the volume and mute independently", async () => {
      const { session } = await playable();
      const [engine] = audio.instances;

      update(session, { volume: 0.3 });
      update(session, { muted: true, volume: 0.3 });
      update(session, { muted: false, volume: 0.3 });
      expect(engine?.setVolume.mock.calls).toEqual([
        [0.3, false],
        [0.3, true],
        [0.3, false],
      ]);
    });

    describe("rate", () => {
      it("keeps the time while the silent clock changes speed", async () => {
        let nowMs = 0;
        vi.spyOn(performance, "now").mockImplementation(() => nowMs);
        const { preview, session, worker } = await playable({}, null);
        const epochAt = (ms: number) => performance.timeOrigin + ms;
        preview.play();

        nowMs = 1_000;
        update(session, { rate: 2 });
        const faster = worker.ofType("clock").at(-1)?.clock;
        expect(faster).toMatchObject({ originMs: 1_000, rate: 2 });
        expect(clockPositionMs(faster!, epochAt(1_500))).toBeCloseTo(2_000);

        nowMs = 1_500;
        update(session, { rate: 0.5 });
        const slower = worker.ofType("clock").at(-1)?.clock;
        expect(slower).toMatchObject({ originMs: 2_000, rate: 0.5 });
        expect(clockPositionMs(slower!, epochAt(1_500))).toBeCloseTo(2_000);
      });

      it("keeps following the audio, which changes speed with it", async () => {
        const { preview, session, worker } = await playable();
        const [engine] = audio.instances;
        preview.play();
        audio.clockRunning = true;
        audio.positionMs = 500;

        update(session, { rate: 2 });
        expect(engine?.setRate).toHaveBeenCalledWith(2);
        expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 500, rate: 2 });

        audio.positionMs = 900;
        engine?.options.onClockChange();
        expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 900, rate: 2 });
      });

      it("plays at the rate it was set to when paused", async () => {
        let nowMs = 0;
        vi.spyOn(performance, "now").mockImplementation(() => nowMs);
        const { preview, session, worker } = await playable({ rate: 4 }, null);
        expect(worker.ofType("clock")).toEqual([]);

        preview.play();
        expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ rate: 4 });
        update(session, { rate: 4 });
        nowMs = 250;
        preview.pause();
        expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 1_000, rate: 0 });
      });
    });

    describe("changing loop or range", () => {
      const epochAt = (ms: number) => performance.timeOrigin + ms;

      it("keeps the position a looping playback had reached when loop is turned off", async () => {
        let nowMs = 0;
        vi.spyOn(performance, "now").mockImplementation(() => nowMs);
        const { preview, session, worker } = await playable({ loop: true }, null);
        preview.play();
        nowMs = 15_000;

        update(session, { loop: false });
        expect(worker.ofType("seek")).toEqual([]);
        const { clock } = worker.ofType("set-playback").at(-1) as { clock: PreviewClock };
        expect(clockPositionMs(clock, epochAt(15_000))).toBeCloseTo(5_000);
        expect(clock.rate).toBe(1);
      });

      it("keeps the position when the range gets a different length", async () => {
        let nowMs = 0;
        vi.spyOn(performance, "now").mockImplementation(() => nowMs);
        const { preview, session, worker } = await playable({ loop: true }, null);
        preview.play();
        nowMs = 25_000;

        update(session, { loop: true, range: { in: 0, out: 60 } });
        const { clock } = worker.ofType("set-playback").at(-1) as { clock: PreviewClock };
        expect(clockPositionMs(clock, epochAt(25_000))).toBeCloseTo(5_000);
        expect(worker.ofType("seek")).toEqual([]);
      });

      it("keeps a paused position, and moves one outside the new range to its start", async () => {
        const { preview, session, worker } = await playable();
        void preview.seek(30);

        update(session, { range: { in: 0, out: 50 } });
        expect(worker.ofType("set-playback").at(-1)?.clock).toMatchObject({
          originMs: 3_000,
          rate: 0,
        });
        expect(worker.ofType("seek")).toHaveLength(1);

        update(session, { range: { in: 40, out: 60 } });
        expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 4_000, rate: 0 });
      });
    });

    describe("invalid values", () => {
      it("leaves every input as it was, and loads nothing, when an update is rejected", async () => {
        const onError = vi.fn();
        const onEnded = vi.fn();
        const { preview, session, worker } = await playable({ lookaheadCount: 8, onError });
        const posted = worker.posted.length;

        session.setInputs({
          composition: composition("replacement"),
          lookaheadCount: 2,
          onEnded,
          onError: vi.fn(),
          rate: 9,
        });
        session.setInputs({ rate: 9 });
        expect(worker.posted.slice(posted)).toEqual([]);
        expect(onError).toHaveBeenCalledTimes(1);
        expect(preview.getSnapshot()).toMatchObject({
          errorCode: "invalid_option",
          isLoaded: true,
        });

        preview.play();
        ended(worker);
        expect(onEnded).not.toHaveBeenCalled();
      });

      it("keeps the error while an earlier load completes, and reports the same value after a valid update", async () => {
        const onError = vi.fn();
        const { preview, session, worker } = await playable({ onError });
        update(session, { composition: composition("second"), onError });
        update(session, { composition: composition("second"), onError, rate: 9 });
        expect(onError).toHaveBeenCalledTimes(1);

        const { loadId } = worker.ofType("load").at(-1) as { loadId: number };
        worker.emit({
          audioTimelineJson: null,
          fps: 10,
          height: 720,
          loadId,
          totalFrames: 100,
          type: "loaded",
          width: 1280,
        });
        expect(preview.getSnapshot()).toMatchObject({ errorCode: "invalid_option" });

        update(session, { composition: composition("second"), onError, rate: 2 });
        expect(preview.getSnapshot().errorCode).toBeNull();
        update(session, { composition: composition("second"), onError, rate: 9 });
        expect(onError).toHaveBeenCalledTimes(2);
      });

      it.each([
        [{ rate: 0.1 }, "rate"],
        [{ rate: 4.5 }, "rate"],
        [{ rate: Number.POSITIVE_INFINITY }, "rate"],
        [{ volume: -0.1 }, "volume"],
        [{ volume: 1.1 }, "volume"],
        [{ volume: Number.NaN }, "volume"],
        [{ range: { in: 5, out: 5 } }, "range.in must be less than range.out"],
        [{ range: { in: 6, out: 5 } }, "range.in must be less than range.out"],
        [{ range: { in: -1, out: 5 } }, "whole frame numbers"],
        [{ range: { in: 1.5, out: 5 } }, "whole frame numbers"],
      ] satisfies [Inputs, string][])(
        "rejects %j, and keeps what was in effect",
        async (invalid, message) => {
          const onError = vi.fn();
          const { preview, session, worker } = await playable({ onError, rate: 2 });

          update(session, { ...invalid, loop: true, onError });
          expect(preview.getSnapshot()).toMatchObject({ errorCode: "invalid_option" });
          expect(preview.getSnapshot().error).toContain(message);
          expect(onError).toHaveBeenCalledWith(expect.objectContaining({ code: "invalid_option" }));
          expect(session.rate).toBe(2);
          expect(session.loop).toBe(false);
          expect(worker.ofType("set-playback")).toEqual([]);
        },
      );

      it("reports each problem once, and clears it when the options are valid", async () => {
        const onError = vi.fn();
        const { preview, session } = await playable({ onError });

        update(session, { onError, rate: 9 });
        update(session, { onError, rate: 9 });
        expect(onError).toHaveBeenCalledTimes(1);

        update(session, { onError, rate: 8 });
        expect(onError).toHaveBeenCalledTimes(2);

        update(session, { onError, rate: 2 });
        expect(onError).toHaveBeenCalledTimes(2);
        expect(preview.getSnapshot()).toMatchObject({ error: null, errorCode: null });
        expect(session.rate).toBe(2);
      });
    });

    describe("range", () => {
      it("opens on the first frame of the range, and keeps seeks inside it", async () => {
        const { preview, session, worker } = await playable({ range: { in: 20, out: 50 } });
        const [engine] = audio.instances;
        expect(preview.getSnapshot().frame).toBe(20);
        expect(engine?.setRange).toHaveBeenLastCalledWith({
          endMs: 5_000,
          loop: false,
          startMs: 2_000,
        });

        void preview.seek(5);
        expect(preview.getSnapshot().frame).toBe(20);
        expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 2_000 });
        void preview.seek(80);
        expect(preview.getSnapshot().frame).toBe(49);
        expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 4_900 });

        update(session);
        void preview.seek(80);
        expect(preview.getSnapshot().frame).toBe(80);
        expect(engine?.setRange).toHaveBeenLastCalledWith({
          endMs: 10_000,
          loop: false,
          startMs: 0,
        });
      });

      it("moves a playhead outside a new range to its start, and leaves one inside", async () => {
        const { preview, session, worker } = await playable();
        void preview.seek(70);
        preview.play();
        const seeks = worker.ofType("seek").length;

        update(session, { range: { in: 60, out: 90 } });
        expect(worker.ofType("seek")).toHaveLength(seeks);

        update(session, { range: { in: 20, out: 50 } });
        expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 2_000 });
        expect(preview.getSnapshot()).toMatchObject({ frame: 20, isPlaying: true });
      });

      it("cuts an out past the end with a warning, and ignores an in past it", async () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
        const { session } = await playable({ range: { in: 20, out: 500 } });
        const [engine] = audio.instances;
        expect(warn).toHaveBeenCalledTimes(1);
        expect(String(warn.mock.calls[0]?.[0])).toContain("range.out is 500");
        expect(engine?.setRange).toHaveBeenLastCalledWith({
          endMs: 10_000,
          loop: false,
          startMs: 2_000,
        });

        update(session, { range: { in: 20, out: 500 }, loop: true });
        expect(warn).toHaveBeenCalledTimes(1);

        update(session, { range: { in: 200, out: 300 } });
        expect(warn).toHaveBeenCalledTimes(2);
        expect(String(warn.mock.calls[1]?.[0])).toContain("range.in is 200");
        expect(engine?.setRange).toHaveBeenLastCalledWith({
          endMs: 10_000,
          loop: false,
          startMs: 0,
        });
      });

      it("fits the range to a composition that loads later", async () => {
        const { preview, session, worker } = await playable({ range: { in: 20, out: 50 } });
        void preview.seek(30);

        session.setInputs({ composition: composition("second"), range: { in: 20, out: 50 } });
        worker.emit({
          audioTimelineJson: audioTimeline(),
          fps: 10,
          height: 720,
          loadId: worker.ofType("load").at(-1)?.loadId as number,
          totalFrames: 40,
          type: "loaded",
          width: 1280,
        });
        expect(audio.instances[0]?.setRange).toHaveBeenLastCalledWith({
          endMs: 4_000,
          loop: false,
          startMs: 2_000,
        });
        expect(preview.getSnapshot().frame).toBe(30);
      });
    });

    describe("ended", () => {
      it("stops on the last frame and says so once", async () => {
        const onEnded = vi.fn();
        const { preview, worker } = await playable({ onEnded });
        preview.play();

        ended(worker);
        expect(onEnded).toHaveBeenCalledTimes(1);
        expect(preview.getSnapshot()).toMatchObject({ frame: 99, isPlaying: false });
        expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 9_900, rate: 0 });
        expect(audio.instances[0]?.pause).toHaveBeenCalled();

        ended(worker);
        expect(onEnded).toHaveBeenCalledTimes(1);
      });

      it("ignores an ending while paused, or one a newer seek has passed", async () => {
        const onEnded = vi.fn();
        const { preview, worker } = await playable({ onEnded });
        ended(worker);
        expect(onEnded).not.toHaveBeenCalled();

        preview.play();
        void preview.seek(10);
        ended(worker, 0);
        expect(onEnded).not.toHaveBeenCalled();
        expect(preview.getSnapshot().isPlaying).toBe(true);
      });

      it("ignores an ending from a range that has since changed", async () => {
        const onEnded = vi.fn();
        const { preview, session, worker } = await playable({
          onEnded,
          range: { in: 0, out: 50 },
        });
        preview.play();
        const queued = revisionOf(worker);

        update(session, { onEnded, range: { in: 0, out: 100 } });
        expect(worker.ofType("seek")).toEqual([]);
        ended(worker, 0, queued);
        expect(onEnded).not.toHaveBeenCalled();
        expect(preview.getSnapshot()).toMatchObject({ frame: 0, isPlaying: true });

        ended(worker);
        expect(onEnded).toHaveBeenCalledTimes(1);
        expect(preview.getSnapshot()).toMatchObject({ frame: 99, isPlaying: false });
      });

      it("ignores an ending queued before a rate change, which carries on playing", async () => {
        const onEnded = vi.fn();
        const { preview, session, worker } = await playable({ onEnded, rate: 4 });
        preview.play();
        const queued = revisionOf(worker);

        update(session, { onEnded, rate: 0.25 });
        ended(worker, 0, queued);
        expect(onEnded).not.toHaveBeenCalled();
        expect(preview.getSnapshot().isPlaying).toBe(true);
        expect(revisionOf(worker)).toBeGreaterThan(queued);

        ended(worker);
        expect(onEnded).toHaveBeenCalledTimes(1);
      });

      it("ignores an ending queued before a pause and resume", async () => {
        const onEnded = vi.fn();
        const { preview, worker } = await playable({ onEnded });
        preview.play();
        const queued = revisionOf(worker);

        preview.pause();
        preview.play();
        ended(worker, 0, queued);
        expect(onEnded).not.toHaveBeenCalled();
        expect(preview.getSnapshot().isPlaying).toBe(true);

        ended(worker);
        expect(onEnded).toHaveBeenCalledTimes(1);
      });

      it("plays again from the start of the range after it ended", async () => {
        const { preview, worker } = await playable({
          onEnded: () => {},
          range: { in: 20, out: 50 },
        });
        preview.play();
        ended(worker);
        expect(preview.getSnapshot()).toMatchObject({ frame: 49, isPlaying: false });

        preview.play();
        expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 2_000 });
        expect(preview.getSnapshot()).toMatchObject({ frame: 20, isPlaying: true });
      });

      it("starts a paused playhead on the last frame again", async () => {
        const { preview, worker } = await playable();
        void preview.seek(99);
        preview.play();
        expect(worker.ofType("seek").at(-1)?.clock).toMatchObject({ originMs: 0 });
        expect(preview.getSnapshot()).toMatchObject({ frame: 0, isPlaying: true });
      });

      it("keeps playing past the end when it loops", async () => {
        let nowMs = 0;
        vi.spyOn(performance, "now").mockImplementation(() => nowMs);
        const { preview, session, worker } = await playable({ loop: true }, null);
        preview.play();
        nowMs = 25_000;
        preview.pause();
        expect(worker.ofType("clock").at(-1)?.clock).toMatchObject({ originMs: 5_000, rate: 0 });
        expect(session.loop).toBe(true);
      });
    });

    describe("time updates", () => {
      it("is called once for each frame shown, until it's removed", async () => {
        const onTimeUpdate = vi.fn();
        const { preview, session, worker } = await playable({ onTimeUpdate });

        showFrame(worker, 3);
        showFrame(worker, 4);
        expect(onTimeUpdate.mock.calls).toEqual([
          [{ frame: 3, timeMs: 300 }],
          [{ frame: 4, timeMs: 400 }],
        ]);

        void preview.seek(10);
        showFrame(worker, 7, 0);
        expect(onTimeUpdate).toHaveBeenCalledTimes(2);
        showFrame(worker, 10, 1);
        expect(onTimeUpdate).toHaveBeenCalledTimes(3);

        update(session, { onTimeUpdate: null });
        showFrame(worker, 11, 1);
        expect(onTimeUpdate).toHaveBeenCalledTimes(3);
      });

      it("isn't called again when a seek is answered on the frame already shown", async () => {
        const onTimeUpdate = vi.fn();
        const { preview, worker } = await playable({ onTimeUpdate });
        showFrame(worker, 10);
        expect(onTimeUpdate).toHaveBeenCalledTimes(1);

        const first = preview.seek(10);
        const second = preview.seek(10);
        showFrame(worker, 10, 1);
        showFrame(worker, 10, 2);
        await expect(Promise.all([first, second])).resolves.toEqual([true, true]);
        expect(onTimeUpdate).toHaveBeenCalledTimes(1);

        void preview.seek(11);
        showFrame(worker, 11, 3);
        expect(onTimeUpdate).toHaveBeenCalledTimes(2);
      });

      it("is called again for a frame of a composition that replaced the one shown", async () => {
        const onTimeUpdate = vi.fn();
        const { session, worker } = await playable({ onTimeUpdate });
        showFrame(worker, 10);

        update(session, { composition: composition("second"), onTimeUpdate });
        const { loadId } = worker.ofType("load").at(-1) as { loadId: number };
        worker.emit({
          audioTimelineJson: audioTimeline(),
          fps: 10,
          height: 720,
          loadId,
          totalFrames: 100,
          type: "loaded",
          width: 1280,
        });
        showFrame(worker, 10);
        expect(onTimeUpdate).toHaveBeenCalledTimes(2);
      });
    });
  });
});
