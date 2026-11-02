import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const QUANTUM = 128;

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
  const { LumenAudioWorkletProcessor } = await import("./audio-worklet.js");
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
    worklet.handleMessage({ generation: 1, holdOnUnderrun: true, startSample: 500, type: "play" });

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
    worklet.handleMessage({ generation: 1, holdOnUnderrun: true, startSample: 0, type: "play" });
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
    expect(resumed?.every((sample) => sample === 0.75)).toBe(true);
    expect(reports.at(-1)).toMatchObject({ stalled: false });
  });

  it("plays silence through missing samples without losing its place when not holding", async () => {
    const { chunk, render, reports, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 1, holdOnUnderrun: false, startSample: 0, type: "play" });

    const [missing] = render();
    expect(missing?.every((sample) => sample === 0)).toBe(true);
    expect(worklet.currentSample).toBe(QUANTUM);
    expect(reports.at(-1)).toMatchObject({ currentSample: QUANTUM, stalled: false });

    chunk(QUANTUM + 10, 12_000, 0.75);
    const [partial] = render();
    expect(partial?.slice(0, 10).every((sample) => sample === 0)).toBe(true);
    expect(partial?.slice(10).every((sample) => sample === 0.75)).toBe(true);
    expect(worklet.currentSample).toBe(QUANTUM * 2);
  });

  it("drops chunks from an older generation", async () => {
    const { chunk, render, worklet } = await createWorklet();
    worklet.handleMessage({ generation: 2, holdOnUnderrun: true, startSample: 0, type: "play" });
    chunk(0, 12_000, 0.5);

    const [left] = render();
    expect(left?.every((sample) => sample === 0)).toBe(true);
    expect(worklet.currentSample).toBe(0);
  });
});
