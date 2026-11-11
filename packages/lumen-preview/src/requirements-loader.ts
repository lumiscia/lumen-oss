import { mod, rangeFrame, type PlaybackRange } from "./playback-range.js";

type FrameRequirementSource = {
  durationFrames: () => number;
  load: (requirementsJson: string) => Promise<void>;
  requirements: (frame: number) => string;
};

export class FrameRequirementLoader {
  #generation = 0;
  #lookaheadCount = 8;
  readonly #pending = new Map<number, Promise<void>>();
  #range: PlaybackRange | null = null;
  readonly #source: FrameRequirementSource;

  constructor(source: FrameRequirementSource) {
    this.#source = source;
  }

  reset(): void {
    this.#generation += 1;
    this.#pending.clear();
  }

  /** Keeps lookahead inside the range being played, wrapping round it only when it loops. */
  setRange(range: PlaybackRange | null): void {
    this.#range = range;
    this.reset();
  }

  setLookaheadCount(lookaheadCount: number): void {
    this.#lookaheadCount = Math.max(0, Math.floor(lookaheadCount));
    this.reset();
  }

  async loadFrame(frame: number): Promise<void> {
    await this.#load(frame);
  }

  prefetchWindow(frame: number): void {
    if (this.#source.durationFrames() <= 0) {
      return;
    }

    void this.#prefetchFrames(frame).catch(() => {
      // The foreground render path reports persistent media errors.
    });
  }

  async #prefetchFrames(frame: number): Promise<void> {
    const generation = this.#generation;
    for (let offset = 0; offset < this.#lookaheadCount; offset += 1) {
      if (generation !== this.#generation) {
        return;
      }
      const next = this.#lookaheadFrame(frame + offset);
      if (next === null) {
        return;
      }
      await this.loadFrame(next);
    }
  }

  async #load(frame: number): Promise<void> {
    const normalizedFrame = this.#normalizeFrame(frame);
    const existing = this.#pending.get(normalizedFrame);
    if (existing) {
      const generation = this.#generation;
      await existing;
      // The joined load may have finished with frames evicted since it uploaded them.
      if (generation === this.#generation) {
        await this.#source.load(this.#source.requirements(normalizedFrame));
      }
      return;
    }

    const generation = this.#generation;
    const pending = (async () => {
      const requirementsJson = this.#source.requirements(normalizedFrame);
      if (generation === this.#generation) {
        await this.#source.load(requirementsJson);
      }
    })().finally(() => {
      if (this.#pending.get(normalizedFrame) === pending) {
        this.#pending.delete(normalizedFrame);
      }
    });
    this.#pending.set(normalizedFrame, pending);

    try {
      await pending;
    } catch (error) {
      if (generation === this.#generation) {
        throw error;
      }
    }
  }

  /** Null once the lookahead runs past the end of a range that doesn't loop. */
  #lookaheadFrame(frame: number): number | null {
    const range = this.#range;
    if (!range) {
      return frame;
    }
    return !range.loop && frame >= range.outFrame ? null : rangeFrame(frame, range);
  }

  #normalizeFrame(frame: number): number {
    const totalFrames = this.#source.durationFrames();
    if (totalFrames <= 0) {
      return frame;
    }

    return mod(frame, totalFrames);
  }
}
