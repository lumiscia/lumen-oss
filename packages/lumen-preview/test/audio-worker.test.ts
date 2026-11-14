import { describe, expect, it } from "vitest";

import type { AudioEngineTimeline } from "../src/audio-engine.js";
import { mixTimelineChunk } from "../src/audio-worker.js";

type MixFixture = {
  expected: number[];
  frames: number;
  name: string;
  sources: Record<string, { frames: number; pattern: [number, number][] }>;
  startSample: number;
  timeline: AudioEngineTimeline;
};

// Written by `browser_mixer_fixtures_match_this_mixer` in crates/engine/src/audio.rs.
const fixturesUrl = new URL(
  "../../../crates/engine/tests/fixtures/audio_mix.json",
  import.meta.url,
);
const { default: fixtures } = (await import(fixturesUrl.href, { with: { type: "json" } })) as {
  default: MixFixture[];
};

function stereoSources(fixture: MixFixture): Map<string, Float32Array> {
  return new Map(
    Object.entries(fixture.sources).map(([id, { frames, pattern }]) => [
      id,
      Float32Array.from({ length: frames * 2 }, (_, index) => {
        const frame = pattern[Math.floor(index / 2) % pattern.length];
        return frame?.[index % 2] ?? 0;
      }),
    ]),
  );
}

function expectSamples(actual: Iterable<number>, expected: readonly number[]): void {
  const samples = Array.from(actual);
  expect(samples).toHaveLength(expected.length);
  samples.forEach((sample, index) => {
    expect(Math.abs(sample - (expected[index] ?? Number.NaN)), `sample ${index}`).toBeLessThan(
      1e-6,
    );
  });
}

describe("mixTimelineChunk against the export mixer", () => {
  it("has fixtures", () => {
    expect(fixtures.length).toBeGreaterThan(0);
  });

  describe.each(fixtures)("$name", (fixture) => {
    const sources = stereoSources(fixture);
    const loopSamples = (fixture.timeline.durationFrames / fixture.timeline.fps) * 48_000;

    it("mixes the same samples", () => {
      const mixed = mixTimelineChunk(
        fixture.timeline,
        sources,
        fixture.startSample,
        fixture.frames,
      );
      expectSamples(mixed, fixture.expected);
    });

    it("mixes the same samples split across chunks", () => {
      const firstFrames = Math.floor(fixture.frames / 2);
      const first = mixTimelineChunk(fixture.timeline, sources, fixture.startSample, firstFrames);
      const second = mixTimelineChunk(
        fixture.timeline,
        sources,
        fixture.startSample + firstFrames,
        fixture.frames - firstFrames,
      );
      expectSamples([...first, ...second], fixture.expected);
    });

    it("mixes the same samples on a later loop", () => {
      const mixed = mixTimelineChunk(
        fixture.timeline,
        sources,
        fixture.startSample + 3 * loopSamples,
        fixture.frames,
      );
      expectSamples(mixed, fixture.expected);
    });
  });
});

describe("mixTimelineChunk looping", () => {
  // 1 second timeline: 48_000 samples.
  const timeline = {
    durationFrames: 10,
    fps: 10,
    voices: [{ duration: 96_000, fadeIn: 0, fadeOut: 0, sourceId: "a", sourceStart: 0 }],
    segments: [
      {
        frames: 48_000,
        from: 0,
        gain: [1, 1] as [number, number],
        position: 0,
        start: 0,
        step: 1,
        to: 96_000,
        voice: 0,
      },
    ],
  };
  const sources = new Map([
    ["a", new Float32Array(96_000 * 2).map((_, index) => (Math.floor(index / 2) % 1000) / 2000)],
  ]);

  it("restarts from timeline position 0 when a chunk crosses the loop end", () => {
    const output = mixTimelineChunk(timeline, sources, 47_990, 20);
    const frame = (index: number) => output[index * 2];

    expect(frame(9)).toBeCloseTo((47_999 % 1000) / 2000, 5);
    expect(frame(10)).toBeCloseTo(0, 5);
    expect(frame(15)).toBeCloseTo((5 / 2000) * (5 / 384), 5);
  });

  it("keeps fractional loop lengths in phase across repetitions", () => {
    const fractional = { ...timeline, durationFrames: 3, fps: 29.97 };
    const loopSamples = (3 / 29.97) * 48_000;
    const first = mixTimelineChunk(fractional, sources, 0, 100);
    const later = mixTimelineChunk(fractional, sources, Math.floor(3_000 * loopSamples), 100);

    expect(first.some((sample: number) => sample !== 0)).toBe(true);
    expect(Array.from(later)).toEqual(Array.from(first));
  });
});

describe("mixTimelineChunk below one frame per second", () => {
  it("loops a 0.5 fps timeline after durationFrames / fps seconds", () => {
    // 5 frames at 0.5 fps last 10 seconds: 480_000 samples.
    const timeline = {
      durationFrames: 5,
      fps: 0.5,
      voices: [{ duration: 480_000, fadeIn: 0, fadeOut: 0, sourceId: "a", sourceStart: 0 }],
      segments: [
        {
          frames: 480_000,
          from: 0,
          gain: [1, 1] as [number, number],
          position: 0,
          start: 0,
          step: 1,
          to: 480_000,
          voice: 0,
        },
      ],
    };
    const ramp = new Float32Array(480_000 * 2).map((_, index) => Math.floor(index / 2) / 480_000);
    const sources = new Map([["a", ramp]]);
    const first = mixTimelineChunk(timeline, sources, 100_000, 64);
    const loopLater = mixTimelineChunk(timeline, sources, 580_000, 64);
    const halfLoopLater = mixTimelineChunk(timeline, sources, 340_000, 64);

    expect(first.some((sample: number) => sample !== 0)).toBe(true);
    expect(Array.from(loopLater)).toEqual(Array.from(first));
    expect(Array.from(halfLoopLater)).not.toEqual(Array.from(first));
  });
});

describe("mixTimelineChunk over a range", () => {
  // One second of a ramp on a one second timeline, so each sample says where it came from.
  const timeline: AudioEngineTimeline = {
    durationFrames: 10,
    fps: 10,
    segments: [
      {
        frames: 48_000,
        from: 0,
        gain: [1, 1],
        position: 0,
        start: 0,
        step: 1,
        to: 48_000,
        voice: 0,
      },
    ],
    voices: [{ duration: 48_000, fadeIn: 0, fadeOut: 0, sourceId: "ramp", sourceStart: 0 }],
  };
  const sources = new Map([
    [
      "ramp",
      Float32Array.from({ length: 96_000 }, (_, index) => (Math.floor(index / 2) + 1) / 1e5),
    ],
  ]);
  const whole = (start: number, frames: number) =>
    mixTimelineChunk(timeline, sources, start, frames);
  const range = { endMs: 200, loop: true, startMs: 100 };
  const FRAMES = 256;

  it("repeats a looping range from its start", () => {
    const length = 4_800;
    const second = mixTimelineChunk(timeline, sources, 4_800 + length + 1_000, FRAMES, range);
    expectSamples(second, Array.from(whole(4_800 + 1_000, FRAMES)));
  });

  it("joins the end of a range to its start without a gap", () => {
    const start = 4_800 + 4_800 - 100;
    const joined = mixTimelineChunk(timeline, sources, start, FRAMES, range);
    const expected = [...whole(9_500, 100), ...whole(4_800, FRAMES - 100)];
    expectSamples(joined, expected);
  });

  it("goes silent at the end of a range that doesn't loop", () => {
    const once = { ...range, loop: false };
    const tail = mixTimelineChunk(timeline, sources, 9_600 - 100, FRAMES, once);
    expectSamples(tail, [
      ...whole(9_500, 100),
      ...Array.from({ length: (FRAMES - 100) * 2 }, () => 0),
    ]);
    expectSamples(
      mixTimelineChunk(timeline, sources, 20_000, FRAMES, once),
      Array.from({ length: FRAMES * 2 }, () => 0),
    );
  });

  it("starts a range that doesn't loop where it begins", () => {
    const once = { ...range, loop: false };
    expectSamples(
      mixTimelineChunk(timeline, sources, 6_000, FRAMES, once),
      Array.from(whole(6_000, FRAMES)),
    );
  });
});
