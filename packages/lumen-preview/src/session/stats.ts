export interface LumenPreviewStats {
  frame: number;
  timelineFps: number;
  targetFrameDurationMs: number;
  renderMs: number;
  actualFps: number;
}

export type LumenPreviewStatsCallback = (stats: LumenPreviewStats) => void;

export const EMPTY_PREVIEW_STATS: LumenPreviewStats = {
  frame: 0,
  timelineFps: 0,
  targetFrameDurationMs: 0,
  renderMs: 0,
  actualFps: 0,
};

export class PlaybackFpsMeter {
  #fps = 0;
  #previous: { frame: number; time: number } | null = null;

  sample(frame: number, isPlaying: boolean, now: number = performance.now()): number {
    if (!isPlaying) {
      this.reset();
      return 0;
    }

    const previous = this.#previous;
    this.#previous = { frame, time: now };
    if (!previous) {
      return this.#fps;
    }

    const frameDelta = frame - previous.frame;
    const timeDelta = now - previous.time;
    if (frameDelta > 0 && timeDelta > 0) {
      this.#fps = (frameDelta * 1_000) / timeDelta;
    }

    return this.#fps;
  }

  reset(): void {
    this.#fps = 0;
    this.#previous = null;
  }
}
