export interface LumenPreviewStats {
  frame: number;
  timelineFps: number;
  targetFrameDurationMs: number;
  renderMs: number;
  actualFps: number;
}

export type LumenPreviewStatsCallback = (stats: LumenPreviewStats) => void;

/** The frame the preview just showed, and its start time in the timeline. */
export interface LumenPreviewTimeUpdate {
  frame: number;
  timeMs: number;
}

export type LumenPreviewTimeUpdateCallback = (update: LumenPreviewTimeUpdate) => void;

export const EMPTY_PREVIEW_STATS: LumenPreviewStats = {
  frame: 0,
  timelineFps: 0,
  targetFrameDurationMs: 0,
  renderMs: 0,
  actualFps: 0,
};

const FPS_WINDOW_MS = 1_000;

/** Counts the distinct frames presented over the last second. */
export class PlaybackFpsMeter {
  #fps = 0;
  #frame: number | null = null;
  #times: number[] = [];

  sample(frame: number, isPlaying: boolean, now: number = performance.now()): number {
    if (!isPlaying) {
      return this.#fps;
    }

    if (frame !== this.#frame) {
      this.#frame = frame;
      this.#times.push(now);
    }

    const windowStart = now - FPS_WINDOW_MS;
    while (this.#times.length > 1 && (this.#times[0] ?? now) < windowStart) {
      this.#times.shift();
    }

    const first = this.#times[0];
    const last = this.#times.at(-1);
    if (first !== undefined && last !== undefined && last > first) {
      this.#fps = ((this.#times.length - 1) * FPS_WINDOW_MS) / (last - first);
    }

    return this.#fps;
  }

  reset(): void {
    this.#fps = 0;
    this.#frame = null;
    this.#times = [];
  }
}
