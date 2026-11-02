import { describe, expect, it } from "vitest";

import { mixTimelineChunk } from "../src/audio-worker.js";

type MixFixture = {
  expected: number[];
  frames: number;
  name: string;
  sources: Record<string, { frames: number; pattern: [number, number][] }>;
  startSample: number;
  timeline: { durationFrames: number; fps: number };
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
    tracks: [{ id: "t", muted: false, solo: false, volume: 1 }],
    clips: [
      {
        durationMs: 2_000,
        id: "a",
        sourceId: "a",
        sourceStartMs: 0,
        startMs: 0,
        trackId: "t",
        volume: 1,
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
