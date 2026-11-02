import { describe, expect, it } from "vitest";

import { PlaybackFpsMeter } from "./stats.js";

describe("PlaybackFpsMeter", () => {
  it("counts presented frames rather than timeline speed", () => {
    const meter = new PlaybackFpsMeter();
    let fps = 0;
    for (let index = 0; index < 5; index += 1) {
      fps = meter.sample(index * 3, true, index * 100);
    }
    expect(fps).toBeCloseTo(10);
  });

  it("keeps counting across a loop and ignores repeats of the same frame", () => {
    const meter = new PlaybackFpsMeter();
    const frames = [28, 29, 29, 0, 1];
    let fps = 0;
    frames.forEach((frame, index) => {
      fps = meter.sample(frame, true, index * 50);
    });
    expect(fps).toBeCloseTo((3 * 1_000) / 200);
  });
});
