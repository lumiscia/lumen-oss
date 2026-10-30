import { describe, expect, it } from "vitest";

import {
  audioTimelineFromCompositionJson,
  LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES,
  createLumenAudioSchedule,
  mixTimelineChunk,
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

describe("mixTimelineChunk", () => {
  // 1 second timeline: 48_000 samples.
  const timeline = {
    durationFrames: 10,
    fps: 10,
    tracks: [{ id: "t", muted: false, solo: false, volume: 1 }],
    clips: [
      { ...clip("a", "t"), durationMs: 2_000 },
      { ...clip("b", "t"), durationMs: 10, startMs: 1_000, sourceStartMs: 0 },
    ],
  };
  const sources = new Map([
    ["a", new Float32Array(96_000 * 2).map((_, index) => (Math.floor(index / 2) % 1000) / 2000)],
    ["b", new Float32Array(96_000 * 2).fill(1)],
  ]);

  it("restarts from timeline position 0 when a chunk crosses the loop end", () => {
    const output = mixTimelineChunk(timeline, sources, 47_990, 20);
    const frame = (index: number) => output[index * 2];

    expect(frame(9)).toBeCloseTo((47_999 % 1000) / 2000, 5);
    expect(frame(10)).toBeCloseTo(0, 5);
    expect(frame(15)).toBeCloseTo((5 / 2000) * Math.min(1, 5 / 384), 5);
  });

  it("maps chunks wholly past the first loop to the wrapped position", () => {
    const wrapped = mixTimelineChunk(timeline, sources, 1_000, 100);
    const looped = mixTimelineChunk(timeline, sources, 48_000 * 3 + 1_000, 100);

    expect(Array.from(looped)).toEqual(Array.from(wrapped));
    expect(wrapped.some((value) => value !== 0)).toBe(true);
  });

  it("keeps fractional loop lengths in phase across repetitions", () => {
    const fractional = { ...timeline, durationFrames: 3, fps: 29.97 };
    const loopSamples = (3 / 29.97) * 48_000;
    const first = mixTimelineChunk(fractional, sources, 0, 100);
    const later = mixTimelineChunk(fractional, sources, Math.floor(3_000 * loopSamples), 100);

    expect(Array.from(later)).toEqual(Array.from(first));
  });
});

describe("msToLumenAudioSample", () => {
  it("uses the engine 48 kHz floor conversion", () => {
    expect(msToLumenAudioSample(1_000)).toBe(48_000);
    expect(msToLumenAudioSample(1.5)).toBe(72);
  });
});

describe("audioTimelineFromCompositionJson", () => {
  it("maps the canonical composition audio contract into the preview engine timeline", () => {
    const timeline = audioTimelineFromCompositionJson(
      JSON.stringify({
        timeline: { fps: 24, duration_frames: 120 },
        audio: {
          tracks: [{ id: "music", muted: true, volume: 0.75 }],
          clips: [
            {
              id: "intro",
              source_id: "song",
              track_id: "music",
              start_frame: 12,
              duration_ms: 1_500,
              source_start_seconds: 0.25,
              volume: 0.5,
            },
          ],
        },
      }),
    );

    expect(timeline).toEqual({
      durationFrames: 120,
      fps: 24,
      tracks: [{ id: "music", muted: true, solo: false, volume: 0.75 }],
      clips: [
        {
          durationMs: 1_500,
          id: "intro",
          sourceId: "song",
          sourceStartMs: 250,
          startMs: 500,
          trackId: "music",
          volume: 0.5,
        },
      ],
    });
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
