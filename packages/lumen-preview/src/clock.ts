/** A playback position that moves by itself: `originMs` at `atEpochMs`, advancing at `rate`. */
export interface PreviewClock {
  atEpochMs: number;
  originMs: number;
  rate: number;
}

/** Milliseconds that mean the same on the page and in its workers. */
export function epochNow(): number {
  return performance.timeOrigin + performance.now();
}

export function clockPositionMs(clock: PreviewClock, nowEpochMs: number): number {
  return clock.originMs + (nowEpochMs - clock.atEpochMs) * clock.rate;
}

/** A parked clock past the end of a timeline waits on its last frame. A running one loops. */
export function fitClock(clock: PreviewClock, durationFrames: number, fps: number): PreviewClock {
  const lastFrameMs = (Math.max(durationFrames - 1, 0) * 1_000) / fps;
  return clock.rate === 0 && clock.originMs > lastFrameMs
    ? { ...clock, originMs: lastFrameMs }
    : clock;
}
