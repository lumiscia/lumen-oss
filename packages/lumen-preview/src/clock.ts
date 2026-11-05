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
