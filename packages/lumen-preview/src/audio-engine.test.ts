import { describe, expect, it } from "vitest";

import {
  LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES,
  createLumenAudioSchedule,
  msToLumenAudioSample,
} from "./audio-engine.js";

describe("createLumenAudioSchedule", () => {
  it("uses Lumen sample timing for offsets, durations, and fades", () => {
    const [scheduled] = createLumenAudioSchedule(
      {
        durationFrames: 120,
        fps: 24,
        tracks: [{ id: "music", muted: false, solo: false, volume: 0.5 }],
        clips: [
          {
            durationMs: 1_000,
            id: "clip",
            sourceId: "song",
            sourceStartMs: 250,
            startMs: 500,
            trackId: "music",
            volume: 0.25,
          },
        ],
      },
      750,
    );

    expect(scheduled).toMatchObject({
      delaySeconds: 0,
      durationSeconds: 0.75,
      fadeSeconds: LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES / 48_000,
      gain: 0.125,
      sourceOffsetSeconds: 0.5,
    });
  });

  it("matches engine solo and mute filtering", () => {
    const scheduled = createLumenAudioSchedule(
      {
        durationFrames: 1,
        fps: 24,
        tracks: [
          { id: "solo", muted: false, solo: true, volume: 1 },
          { id: "muted", muted: true, solo: false, volume: 1 },
          { id: "other", muted: false, solo: false, volume: 1 },
        ],
        clips: [clip("a", "solo"), clip("b", "muted"), clip("c", "other")],
      },
      0,
    );

    expect(scheduled.map((entry) => entry.clip.id)).toEqual(["a"]);
  });
});

describe("msToLumenAudioSample", () => {
  it("uses the engine 48 kHz floor conversion", () => {
    expect(msToLumenAudioSample(1_000)).toBe(48_000);
    expect(msToLumenAudioSample(1.5)).toBe(72);
  });
});

function clip(id: string, trackId: string) {
  return {
    durationMs: 1_000,
    id,
    sourceId: id,
    sourceStartMs: 0,
    startMs: 0,
    trackId,
    volume: 1,
  };
}
