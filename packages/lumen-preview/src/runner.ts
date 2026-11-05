import { clockPositionMs, type PreviewClock } from "./clock.js";

// A correction this close to the running clock is jitter, and following it could step a frame back.
const CLOCK_TOLERANCE_MS = 30;
// Frame times round-trip through floats, and a frame must not land just below its own start.
const FRAME_EPSILON = 1e-6;

export interface PreviewRunnerRenderer {
  /** Moves the playhead, which protects what that frame needs from eviction. */
  setFrame(frame: number): void;
  /** Resolves once the playhead's media is loaded. */
  prepareFrame(): Promise<void>;
  renderFrame(): Promise<void>;
}

export interface PreviewRunnerHost<TTimer> {
  clearTimer(timer: TTimer): void;
  now(): number;
  onFailure(error: unknown): void;
  /** `renderMs` is null when the frame was already on screen. */
  onFrame(frame: number, seekId: number, renderMs: number | null): void;
  setTimer(callback: () => void, delayMs: number): TTimer;
}

/**
 * Keeps the canvas on the frame its clock points at, looping over the timeline.
 *
 * One loop owns every render, so renders never overlap. Whatever falls behind doesn't stall the
 * clock: the frame in hand is shown, then the next render jumps to wherever the clock is.
 */
export class PreviewRunner<TTimer> {
  #acknowledgedSeekId = 0;
  #clock: PreviewClock = { atEpochMs: 0, originMs: 0, rate: 0 };
  #dirty = false;
  #durationFrames = 0;
  #failed = false;
  #fps = 1;
  // Bumped by every load, so nothing started for an older timeline can land on the new one.
  #generation = 0;
  readonly #host: PreviewRunnerHost<TTimer>;
  #rendered: number | null = null;
  readonly #renderer: PreviewRunnerRenderer;
  #running = false;
  #seekId = 0;
  #timer: TTimer | null = null;

  constructor(renderer: PreviewRunnerRenderer, host: PreviewRunnerHost<TTimer>) {
    this.#renderer = renderer;
    this.#host = host;
  }

  /** Starts a newly loaded timeline, parked on frame 0. */
  load(durationFrames: number, fps: number): void {
    this.#generation += 1;
    this.#clock = { atEpochMs: this.#host.now(), originMs: 0, rate: 0 };
    this.#dirty = true;
    this.#durationFrames = durationFrames;
    this.#failed = false;
    this.#fps = fps;
    this.#rendered = null;
    this.#wake();
  }

  unload(): void {
    this.#generation += 1;
    this.#durationFrames = 0;
    this.#cancelTimer();
  }

  setClock(clock: PreviewClock): void {
    const drift = this.#wrappedMs(clock.originMs - clockPositionMs(this.#clock, clock.atEpochMs));
    if (clock.rate === this.#clock.rate && Math.abs(drift) < CLOCK_TOLERANCE_MS) {
      return;
    }
    this.#clock = clock;
    // The pause that answers a failure is no reason to try that frame again, but playing is.
    this.#failed &&= clock.rate === 0;
    this.#wake();
  }

  /** The newest seek wins, and one that lands on the frame on screen is answered without a render. */
  seek(seekId: number, clock: PreviewClock): void {
    this.#clock = clock;
    this.#failed = false;
    this.#seekId = seekId;
    this.#wake();
  }

  #wake(): void {
    this.#cancelTimer();
    void this.#run();
  }

  async #run(): Promise<void> {
    if (this.#running || this.#durationFrames === 0 || this.#failed) {
      return;
    }
    const frame = this.#targetFrame();
    if (frame === this.#rendered && !this.#dirty) {
      this.#answerSeek(frame);
      this.#scheduleNextFrame();
      return;
    }
    this.#running = true;
    await this.#render(frame);
    this.#running = false;
    // A render can resolve without leaving the task, so a composition that can't keep up would
    // never let a pause or seek in. Going through a timer yields to them on every render.
    this.#schedule(0);
  }

  async #render(frame: number): Promise<void> {
    const generation = this.#generation;
    const seekId = this.#seekId;
    const isStale = () => generation !== this.#generation || seekId !== this.#seekId;
    const startedAt = this.#host.now();
    this.#dirty = false;
    try {
      this.#renderer.setFrame(frame);
      await this.#renderer.prepareFrame();
      if (isStale()) {
        return;
      }
      await this.#renderer.renderFrame();
    } catch (error) {
      if (!isStale()) {
        this.#failed = true;
        this.#host.onFailure(error);
      }
      return;
    }
    if (generation === this.#generation) {
      this.#rendered = frame;
      this.#acknowledgedSeekId = seekId;
      this.#host.onFrame(frame, seekId, this.#host.now() - startedAt);
    }
  }

  #answerSeek(frame: number): void {
    if (this.#acknowledgedSeekId !== this.#seekId) {
      this.#acknowledgedSeekId = this.#seekId;
      this.#host.onFrame(frame, this.#seekId, null);
    }
  }

  #scheduleNextFrame(): void {
    if (this.#clock.rate <= 0) {
      return;
    }
    const positionMs = clockPositionMs(this.#clock, this.#host.now());
    const nextFrameMs = ((this.#framePosition(positionMs) + 1) * 1_000) / this.#fps;
    this.#schedule(Math.max((nextFrameMs - positionMs) / this.#clock.rate, 1));
  }

  #schedule(delayMs: number): void {
    this.#timer = this.#host.setTimer(() => {
      this.#timer = null;
      void this.#run();
    }, delayMs);
  }

  #cancelTimer(): void {
    if (this.#timer !== null) {
      this.#host.clearTimer(this.#timer);
      this.#timer = null;
    }
  }

  #targetFrame(): number {
    const positionMs = clockPositionMs(this.#clock, this.#host.now());
    return this.#framePosition(positionMs) % this.#durationFrames;
  }

  #framePosition(positionMs: number): number {
    return Math.floor((Math.max(positionMs, 0) * this.#fps) / 1_000 + FRAME_EPSILON);
  }

  /** The shortest way round the loop, because the audio clock wraps where this clock keeps going. */
  #wrappedMs(ms: number): number {
    const loopMs = (this.#durationFrames * 1_000) / this.#fps;
    return loopMs > 0 ? ms - loopMs * Math.round(ms / loopMs) : ms;
  }
}
