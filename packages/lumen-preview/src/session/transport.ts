import type { LumenAudioEngine } from "../audio-engine.js";
import { clockPositionMs, epochNow, fitClock, type PreviewClock } from "../clock.js";
import {
  DEFAULT_PLAYBACK,
  describeRangeMisfit,
  rangeFrame,
  rangeMs,
  resolvePlaybackRange,
  type LumenPreviewRange,
  type PlaybackRange,
  type PlaybackSettings,
} from "../playback-range.js";
import type { LumenPreviewWorkerCommand } from "../worker-host.js";

const CLOCK_CORRECTION_INTERVAL_MS = 250;

/** Playing is `held` while it waits, with its clock parked, for media or the frame it is on. */
export type TransportState = "paused" | "playing" | "held";

export interface TransportHost {
  isLoading(): boolean;
  /** Whether media the composition needs is pending. */
  mediaPending(): boolean;
  post(message: LumenPreviewWorkerCommand): void;
  report(state: { frame?: number; isPlaying?: boolean }): void;
}

interface Timeline {
  fps: number;
  totalFrames: number;
}

/**
 * Playback over time: where the clock is, the range being played, and the audio that leads it.
 * The worker follows the clock it is sent.
 */
export class Transport {
  audio: LumenAudioEngine | null = null;
  clock: PreviewClock = { atEpochMs: 0, originMs: 0, rate: 0 };
  // Whether the worker is still loading the media of the frame playback is on.
  frameWaiting = false;
  // A seek made without a loaded timeline, applied once one loads.
  pendingSeek: number | null = null;
  // The last options that were valid, which are the ones in effect.
  playback: PlaybackSettings = DEFAULT_PLAYBACK;
  // The part of the loaded timeline being played.
  range: PlaybackRange | null = null;
  // Counts the changes that make a queued `ended` stale: loop, range, rate, pause and play. The
  // worker follows it through the clocks it is sent, and its `ended` events name the one they ended under.
  revision = 0;
  // The next worker counts its seeks from zero.
  seekId = 0;
  state: TransportState = "paused";
  timeline: Timeline | null = null;
  #correctionTimer: ReturnType<typeof setInterval> | null = null;
  #pendingSeeks = new Map<number, (presented: boolean) => void>();
  // What was last said about a range that didn't fit. The same message isn't reported again, so a
  // callback that re-renders can't loop.
  #rangeMisfit: string | null = null;
  readonly #host: TransportHost;

  constructor(host: TransportHost) {
    this.#host = host;
  }

  get isPlaying(): boolean {
    return this.state !== "paused";
  }

  /** Playing, and not held for media. */
  get running(): boolean {
    return this.state === "playing";
  }

  /** Wrapped into the range, or held at its end, as the worker shows it. */
  positionMs(): number {
    const ms = this.audio?.isPlaying()
      ? this.audio.currentTimeMs()
      : clockPositionMs(this.clock, epochNow());
    return this.range && this.timeline
      ? rangeMs(Math.max(ms, 0), this.range, this.timeline.fps)
      : Math.max(ms, 0);
  }

  /** Applies the playback options that changed. */
  setPlayback(next: PlaybackSettings): void {
    const previous = this.playback;
    this.playback = next;
    if (next.rate !== previous.rate) {
      this.#changeRate(next.rate);
    }
    if (next.loop !== previous.loop || !sameRange(next.range, previous.range)) {
      this.#changeRange();
    }
  }

  play(): void {
    const range = this.range;
    if (!this.timeline || !range || this.isPlaying) {
      return;
    }
    // Playing from the last frame of a range that doesn't loop starts it again.
    if (!range.loop && frameAt(this.positionMs(), this.timeline.fps) >= range.outFrame - 1) {
      void this.seek(range.inFrame);
    }
    this.state = "playing";
    this.revision += 1;
    this.#host.report({ isPlaying: true });
    this.updateHold();
    if (this.running) {
      this.#startClock();
    }
  }

  pause(): void {
    if (!this.isPlaying) {
      return;
    }
    const originMs = this.positionMs();
    this.state = "paused";
    this.revision += 1;
    this.audio?.pause();
    this.park(originMs);
    this.#host.report({ isPlaying: false });
  }

  seek(frame: number): Promise<boolean> {
    const timeline = this.timeline;
    if (this.#host.isLoading() || !timeline) {
      // The next load decides how long the composition is, then seeks there.
      this.pendingSeek = Math.max(0, Math.round(frame));
      this.#host.report({ frame: this.pendingSeek });
      return Promise.resolve(false);
    }
    const target = clampFrame(
      Math.max(0, Math.round(frame)),
      this.range ?? { inFrame: 0, loop: false, outFrame: timeline.totalFrames },
    );
    const originMs = (target * 1_000) / timeline.fps;
    this.audio?.seekMs(originMs);
    this.#placeClock(originMs);
    this.#host.report({ frame: target });
    const seekId = this.#postSeek();
    return new Promise((resolve) => this.#pendingSeeks.set(seekId, resolve));
  }

  /**
   * Parks the clock at `originMs`, or at the nearest frame of the range when that is outside it,
   * and tells the worker.
   */
  park(originMs: number): void {
    this.#stopCorrections();
    const parked: PreviewClock = { atEpochMs: epochNow(), originMs, rate: 0 };
    this.clock =
      this.range && this.timeline ? fitClock(parked, this.range, this.timeline.fps) : parked;
    this.#postClock();
  }

  /** Audio leads whenever it's playing. Without it the worker's clock simply keeps running. */
  correctClock(): void {
    if (!this.running) {
      return;
    }
    const audio = this.audio;
    let next: PreviewClock | null = null;
    if (audio?.isPlaying()) {
      next = audioClock(audio, this.playback.rate);
    } else if (this.clock.rate === 0) {
      next = { atEpochMs: epochNow(), originMs: this.clock.originMs, rate: this.playback.rate };
    }
    if (next) {
      this.clock = next;
      this.#postClock();
    }
  }

  setFrameWaiting(waiting: boolean): void {
    this.frameWaiting = waiting;
    this.updateHold();
  }

  /**
   * Playing waits, with its clock parked, while media the composition needs is pending or the
   * frame it is on is still loading.
   */
  updateHold(): void {
    const hold = this.isPlaying && (this.frameWaiting || this.#host.mediaPending());
    if (hold === (this.state === "held")) {
      return;
    }
    this.state = hold ? "held" : "playing";
    this.revision += 1;
    if (hold) {
      const originMs = this.positionMs();
      this.audio?.pause();
      this.park(originMs);
    } else {
      this.#startClock();
    }
  }

  /** Playback ran out of range: it stops on the last frame, as a pause there would. Whether it did. */
  ended(seekId: number, revision: number): boolean {
    const range = this.range;
    const timeline = this.timeline;
    if (seekId !== this.seekId || revision !== this.revision) {
      return false;
    }
    if (!this.isPlaying || !range || !timeline) {
      return false;
    }
    const lastFrame = range.outFrame - 1;
    const lastFrameMs = (lastFrame * 1_000) / timeline.fps;
    this.state = "paused";
    this.audio?.pause();
    this.audio?.seekMs(lastFrameMs);
    this.park(lastFrameMs);
    this.#host.report({ frame: lastFrame, isPlaying: false });
    return true;
  }

  /** Resolves the seeks up to `seekId`, and returns whether that is the newest seek. */
  acknowledgeSeeks(seekId: number): boolean {
    for (const [pendingId, resolve] of this.#pendingSeeks) {
      if (pendingId <= seekId) {
        this.#pendingSeeks.delete(pendingId);
        resolve(true);
      }
    }
    return seekId === this.seekId;
  }

  settleSeeks(presented: boolean): void {
    for (const resolve of this.#pendingSeeks.values()) {
      resolve(presented);
    }
    this.#pendingSeeks.clear();
  }

  /** A new load starts: the seeks asked of the last one are answered, and nothing is loading yet. */
  beginLoad(): void {
    this.settleSeeks(false);
    this.frameWaiting = false;
  }

  /** The composition is gone: playback stops and forgets its position. */
  unload(): void {
    this.state = "paused";
    this.pendingSeek = null;
    this.#stopCorrections();
    this.clock = { atEpochMs: 0, originMs: 0, rate: 0 };
    this.range = null;
    this.timeline = null;
    this.audio?.loadAudioTimeline(null);
  }

  /** The load failed: playback stops where it was, and the timeline is no longer one to play. */
  failLoad(): void {
    this.pause();
    this.range = null;
    this.timeline = null;
  }

  /**
   * The worker starts the new timeline on the clock it already follows, so an unchanged audio
   * timeline needs only the usual correction. A pending seek, or audio restarted for a new
   * timeline, seeks both. Returns the frame the preview is on, given the one it showed before.
   */
  load(
    { audioTimelineJson, fps, totalFrames }: Timeline & { audioTimelineJson: string | null },
    shownFrame: number,
  ): number {
    // Read against the previous timeline, before the audio can be told about the new one.
    const positionMs = this.positionMs();
    const pending = this.pendingSeek;
    this.pendingSeek = null;
    const timeline = { fps, totalFrames };
    this.timeline = timeline;
    const audioChanged =
      this.audio?.loadAudioTimeline(audioTimelineJson, {
        durationFrames: totalFrames,
        fps,
      }) ?? false;

    const range = this.#applyRange(timeline);
    const lastFrame = range.outFrame - 1;
    const lastFrameMs = (lastFrame * 1_000) / fps;
    const restartAudio = audioChanged && this.running;
    let frame = shownFrame;
    if (pending !== null || restartAudio) {
      // Playback carries on round the new range, as the worker would.
      const originMs =
        pending !== null
          ? (clampFrame(pending, range) * 1_000) / fps
          : Math.min(rangeMs(positionMs, range, fps), lastFrameMs);
      if (restartAudio) {
        this.audio?.play(originMs);
      } else {
        this.audio?.seekMs(originMs);
      }
      this.#placeClock(originMs);
      this.#postSeek();
      frame = clampFrame(frameAt(originMs, fps), range);
    } else if (this.running) {
      this.correctClock();
      // The frame on screen stands until the worker shows the next, unless it's outside the range.
      if (frame < range.inFrame || frame > lastFrame) {
        frame = rangeFrame(frameAt(this.positionMs(), fps), range);
      }
    } else {
      // The worker fits its clock the same way.
      this.clock = fitClock(this.clock, range, fps);
      if (audioChanged) {
        this.audio?.seekMs(this.clock.originMs);
      }
      frame = frameAt(this.clock.originMs, fps);
    }
    return frame;
  }

  dispose(): void {
    this.#stopCorrections();
    this.settleSeeks(false);
    this.audio?.dispose();
  }

  /** Playback carries on from where it is, at the new speed. */
  #changeRate(rate: number): void {
    const positionMs = this.positionMs();
    this.audio?.setRate(rate);
    if (!this.running) {
      return;
    }
    this.revision += 1;
    this.clock = this.audio?.isPlaying()
      ? audioClock(this.audio, rate)
      : { atEpochMs: epochNow(), originMs: positionMs, rate };
    this.#postClock();
  }

  /**
   * Playback carries on from the position it had under the old loop and range, which both clocks
   * are rebased to. A playhead outside the new range moves to its start.
   */
  #changeRange(): void {
    const timeline = this.timeline;
    const positionMs = timeline ? this.positionMs() : 0;
    const range = timeline ? this.#applyRange(timeline) : null;
    if (timeline) {
      this.#placeClock(positionMs);
    }
    const { loop, range: requested } = this.playback;
    this.revision += 1;
    this.#host.post({
      clock: this.clock,
      loop,
      range: requested,
      revision: this.revision,
      type: "set-playback",
    });
    if (!timeline || !range) {
      return;
    }
    const frame = frameAt(positionMs, timeline.fps);
    if (frame < range.inFrame || frame >= range.outFrame) {
      void this.seek(range.inFrame);
    } else {
      this.correctClock();
    }
  }

  /** Fits the requested range to the timeline, and tells the audio. */
  #applyRange(timeline: Timeline): PlaybackRange {
    const { loop, range: requested } = this.playback;
    const misfit = describeRangeMisfit(requested, timeline.totalFrames);
    if (misfit !== null && misfit !== this.#rangeMisfit) {
      console.warn(`[LumenPreviewSession] ${misfit}`);
    }
    this.#rangeMisfit = misfit;
    const range = resolvePlaybackRange(requested, loop, timeline.totalFrames);
    this.range = range;
    this.audio?.setRange({
      endMs: (range.outFrame * 1_000) / timeline.fps,
      loop: range.loop,
      startMs: (range.inFrame * 1_000) / timeline.fps,
    });
    return range;
  }

  #startClock(): void {
    this.audio?.play(this.positionMs());
    this.correctClock();
    this.#correctionTimer = setInterval(() => this.correctClock(), CLOCK_CORRECTION_INTERVAL_MS);
  }

  #stopCorrections(): void {
    if (this.#correctionTimer !== null) {
      clearInterval(this.#correctionTimer);
      this.#correctionTimer = null;
    }
  }

  #placeClock(originMs: number): void {
    this.clock = {
      atEpochMs: epochNow(),
      originMs,
      rate: this.running && !this.audio?.isPlaying() ? this.playback.rate : 0,
    };
  }

  #postSeek(): number {
    const seekId = ++this.seekId;
    this.#host.post({ clock: this.clock, seekId, type: "seek" });
    return seekId;
  }

  #postClock(): void {
    this.#host.post({ clock: this.clock, revision: this.revision, type: "clock" });
  }
}

function audioClock(audio: LumenAudioEngine, rate: number): PreviewClock {
  return {
    atEpochMs: epochNow(),
    originMs: audio.currentTimeMs(),
    rate: audio.isClockRunning() ? rate : 0,
  };
}

/** A seek lands inside the range, whether or not it loops. */
function clampFrame(frame: number, range: PlaybackRange): number {
  return Math.max(range.inFrame, Math.min(frame, range.outFrame - 1));
}

function sameRange(left: LumenPreviewRange | null, right: LumenPreviewRange | null): boolean {
  return left?.in === right?.in && left?.out === right?.out;
}

// Matches the worker's rounding, so a frame's own start time doesn't land on the frame before.
function frameAt(positionMs: number, fps: number): number {
  return Math.floor((positionMs * fps) / 1_000 + 1e-6);
}
