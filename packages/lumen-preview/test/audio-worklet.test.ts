import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const QUANTUM = 128;
const HOLD = 48_000;

type Position = {
  contextTime: number;
  currentSample: number;
  generation: number;
  stalled: boolean;
};

async function createWorklet() {
  const reports: Position[] = [];
  vi.stubGlobal(
    "AudioWorkletProcessor",
    class {
      port: { onmessage?: unknown; postMessage: (message: Position) => void } = {
        postMessage: (message) => reports.push(message),
      };
    },
  );
  vi.stubGlobal("registerProcessor", () => {});
  vi.stubGlobal("currentTime", 0);
  vi.resetModules();
  const { LumenAudioWorkletProcessor } = await import("../src/audio-worklet.js");
  const worklet = new LumenAudioWorkletProcessor();
  const render = () => {
    const output = [new Float32Array(QUANTUM), new Float32Array(QUANTUM)];
    worklet.process([], [output]);
    return output;
  };
  const chunk = (startSample: number, frames: number, value: number) =>
    worklet.handleMessage({
      channelCount: 2,
      frames,
      generation: 1,
      sampleRate: 48_000,
      samples: new Float32Array(frames * 2).fill(value),
      startSample,
      type: "chunk",
    });
  return { chunk, render, reports, worklet };
}

describe("LumenAudioWorkletProcessor", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("holds the position until the first chunk arrives", async () => {
    const { chunk, render, reports, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdFrames: HOLD, startSample: 500, type: "play" });

    render();
    render();
    expect(worklet.currentSample).toBe(500);
    expect(reports.at(-1)).toMatchObject({ currentSample: 500, stalled: true });

    chunk(0, 12_000, 0.5);
    const [left] = render();
    expect(left?.every((sample) => sample === 0.5)).toBe(true);
    expect(worklet.currentSample).toBe(500 + QUANTUM);
    expect(reports.at(-1)).toMatchObject({ currentSample: 500 + QUANTUM, stalled: false });
  });

  it("stops at the last available sample during an underrun and resumes from it", async () => {
    const { chunk, render, reports, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdFrames: HOLD, startSample: 0, type: "play" });
    chunk(0, QUANTUM + 10, 0.25);
    render();
    reports.length = 0;

    const [left] = render();
    expect(left?.slice(0, 10).every((sample) => sample === 0.25)).toBe(true);
    expect(left?.slice(10).every((sample) => sample === 0)).toBe(true);
    expect(worklet.currentSample).toBe(QUANTUM + 10);
    expect(reports).toMatchObject([{ currentSample: QUANTUM + 10, stalled: true }]);

    render();
    expect(worklet.currentSample).toBe(QUANTUM + 10);

    chunk(QUANTUM + 10, 12_000, 0.75);
    const [resumed] = render();
    expect(resumed?.[0]).toBe(0);
    expect(resumed?.[QUANTUM - 1]).toBeGreaterThan(0);
    expect(worklet.currentSample).toBe(QUANTUM * 2 + 10);
    expect(reports.at(-1)).toMatchObject({ stalled: false });
  });

  it("ramps up over the samples that recover an underrun, keeping its position", async () => {
    const { chunk, render, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdFrames: HOLD, startSample: 0, type: "play" });
    chunk(0, QUANTUM, 0.25);
    render();
    render();

    chunk(QUANTUM, 12_000, 0.75);
    const quanta = 4;
    const output = Array.from({ length: quanta }, () => render()[0] ?? new Float32Array());
    const ramp = Float32Array.from(output.flatMap((quantum) => [...quantum]));

    const rampFrames = ramp.findIndex((sample) => sample === 0.75);
    expect(ramp[0]).toBe(0);
    expect(rampFrames).toBeGreaterThanOrEqual(QUANTUM);
    expect(rampFrames).toBeLessThan(QUANTUM * 3);
    for (let frame = 1; frame < rampFrames; frame += 1) {
      expect(ramp[frame]).toBeCloseTo((0.75 * frame) / rampFrames, 5);
    }
    expect(ramp.slice(rampFrames).every((sample) => sample === 0.75)).toBe(true);
    expect(worklet.currentSample).toBe(QUANTUM * (1 + quanta));
  });

  it("starts the ramp again when the audio drops out during it", async () => {
    const { chunk, render, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdFrames: HOLD, startSample: 0, type: "play" });
    chunk(0, QUANTUM, 0.5);
    render();
    render();
    chunk(QUANTUM, 20, 0.5);
    render();

    chunk(QUANTUM + 20, 12_000, 0.5);
    const [again] = render();

    expect(again?.[0]).toBe(0);
    expect(again?.[QUANTUM - 1]).toBeLessThan(0.5);
  });

  it("doesn't fade a first play that starts late, wherever it starts", async () => {
    const { chunk, render, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdFrames: HOLD, startSample: 24_000, type: "play" });
    render();
    render();

    chunk(24_000, 12_000, 0.5);
    const [left] = render();

    expect(left?.every((sample) => sample === 0.5)).toBe(true);
  });

  it("doesn't fade a restart after an underrun", async () => {
    const { chunk, render, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdFrames: HOLD, startSample: 0, type: "play" });
    chunk(0, QUANTUM, 0.5);
    render();
    render();

    worklet.handleMessage({ generation: 1, holdFrames: HOLD, startSample: 0, type: "play" });
    render();
    chunk(0, 12_000, 0.5);
    const [left] = render();

    expect(left?.every((sample) => sample === 0.5)).toBe(true);
  });

  it("stops holding when its budget runs out, and moves on in silence", async () => {
    const { chunk, render, reports, worklet } = await createWorklet();
    worklet.handleMessage({
      generation: 1,
      holdFrames: QUANTUM * 2,
      startSample: 1_000,
      type: "play",
    });
    expect(reports.at(-1)).toMatchObject({ currentSample: 1_000, stalled: true });

    render();
    render();
    expect(worklet.currentSample).toBe(1_000);

    render();
    expect(worklet.currentSample).toBe(1_000 + QUANTUM);
    expect(reports.at(-1)).toMatchObject({ stalled: false });

    chunk(1_000, QUANTUM, 0.9);
    const [behind] = render();
    expect(behind?.every((sample) => sample === 0)).toBe(true);
    expect(worklet.currentSample).toBe(1_000 + QUANTUM * 2);
  });

  it("never holds again after its budget has run out, even once it has heard audio", async () => {
    const { chunk, render, reports, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdFrames: 0, startSample: 0, type: "play" });
    expect(reports.at(-1)).toMatchObject({ stalled: false });

    chunk(0, QUANTUM, 0.5);
    render();
    const before = worklet.currentSample;
    render();
    render();
    expect(worklet.currentSample).toBe(before + QUANTUM * 2);
    expect(reports.at(-1)).toMatchObject({ stalled: false });
  });

  it("keeps holding through an underrun when audio started within its budget", async () => {
    const { chunk, render, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdFrames: QUANTUM, startSample: 0, type: "play" });
    chunk(0, QUANTUM, 0.5);
    render();

    for (let quantum = 0; quantum < 10; quantum += 1) {
      render();
    }
    expect(worklet.currentSample).toBe(QUANTUM);
  });

  it("plays the mixer's samples untouched across chunks, silence and a loop", async () => {
    const { render, worklet } = await createWorklet();
    const { mixTimelineChunk } = await import("../src/audio-worker.js");
    const seconds = 1;
    const timeline = {
      clips: [
        {
          durationMs: 500,
          id: "clip",
          sourceId: "tone",
          sourceStartMs: 0,
          startMs: 250,
          trackId: "track",
          volume: 1,
        },
      ],
      durationFrames: 30 * seconds,
      fps: 30,
      tracks: [{ id: "track", muted: false, solo: false, volume: 1 }],
    };
    const sources = new Map([["tone", new Float32Array(96_000).fill(0.5)]]);
    const start = 30_000;
    const frames = 36_000;
    worklet.handleMessage({
      generation: 1,

      startSample: start,
      type: "play",
    });
    for (let chunkStart = start; chunkStart < start + frames; chunkStart += 12_000) {
      worklet.handleMessage({
        channelCount: 2,
        frames: 12_000,
        generation: 1,
        sampleRate: 48_000,
        samples: mixTimelineChunk(timeline, sources, chunkStart, 12_000),
        startSample: chunkStart,
        type: "chunk",
      });
    }

    const expected = mixTimelineChunk(timeline, sources, start, frames);
    expect(new Set(expected).size).toBeGreaterThan(2);
    for (let quantum = 0; quantum < Math.floor(frames / QUANTUM); quantum += 1) {
      const [left] = render();
      for (let frame = 0; frame < QUANTUM; frame += 1) {
        const index = (quantum * QUANTUM + frame) * 2;
        if (left?.[frame] !== expected[index]) {
          expect.fail(`frame ${index / 2}: ${left?.[frame]} != ${expected[index]}`);
        }
      }
    }
  });

  it("drops chunks from an older generation", async () => {
    const { chunk, render, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 2, holdFrames: HOLD, startSample: 0, type: "play" });
    chunk(0, 12_000, 0.5);

    const [left] = render();
    expect(left?.every((sample) => sample === 0)).toBe(true);
    expect(worklet.currentSample).toBe(0);
  });

  describe("at another rate", () => {
    // Sample n holds n / 1000, so the output says which position it read.
    const ramp = (worklet: { handleMessage(message: unknown): void }) =>
      worklet.handleMessage({
        channelCount: 2,
        frames: 12_000,
        generation: 1,
        sampleRate: 48_000,
        samples: Float32Array.from({ length: 24_000 }, (_, index) => Math.floor(index / 2) / 1_000),
        startSample: 0,
        type: "chunk",
      });

    it("reads twice as fast at rate 2, and reports the rate", async () => {
      const { render, reports, worklet } = await createWorklet();
      worklet.handleMessage({
        generation: 1,
        holdFrames: 0,
        rate: 2,
        startSample: 10,
        type: "play",
      });
      ramp(worklet);

      const [left] = render();
      expect(left?.[0]).toBeCloseTo(0.01);
      expect(left?.[1]).toBeCloseTo(0.012);
      expect(left?.[QUANTUM - 1]).toBeCloseTo((10 + 2 * (QUANTUM - 1)) / 1_000);
      expect(worklet.currentSample).toBe(10 + 2 * QUANTUM);
      expect(reports.at(-1)).toMatchObject({ rate: 2 });
    });

    it("interpolates between samples at a slower rate", async () => {
      const { render, worklet } = await createWorklet();
      worklet.handleMessage({
        generation: 1,
        holdFrames: 0,
        rate: 0.5,
        startSample: 10,
        type: "play",
      });
      ramp(worklet);

      const [left] = render();
      expect(left?.[0]).toBeCloseTo(0.01);
      expect(left?.[1]).toBeCloseTo(0.0105);
      expect(left?.[2]).toBeCloseTo(0.011);
      expect(worklet.currentSample).toBe(10 + QUANTUM / 2);
    });

    it("changes rate mid-playback from the position it has reached", async () => {
      const { render, reports, worklet } = await createWorklet();
      worklet.handleMessage({ generation: 1, holdFrames: 0, startSample: 0, type: "play" });
      ramp(worklet);
      render();
      expect(worklet.currentSample).toBe(QUANTUM);

      worklet.handleMessage({ rate: 4, rateRevision: 3, type: "rate" });
      const [left] = render();
      worklet.reportPosition(0);
      expect(reports.at(-1)).toMatchObject({ rate: 4, rateRevision: 3 });
      expect(left?.[0]).toBeCloseTo(QUANTUM / 1_000);
      expect(left?.[1]).toBeCloseTo((QUANTUM + 4) / 1_000);
      expect(worklet.currentSample).toBe(QUANTUM + 4 * QUANTUM);
    });

    it("moves through silence at the rate when a chunk is missing", async () => {
      const { render, worklet } = await createWorklet();
      worklet.handleMessage({
        generation: 1,
        holdFrames: 0,
        rate: 2,
        startSample: 0,
        type: "play",
      });
      render();
      expect(worklet.currentSample).toBe(2 * QUANTUM);
    });
  });
});
