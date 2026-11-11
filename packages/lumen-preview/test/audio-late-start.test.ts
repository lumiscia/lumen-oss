import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LumenAudioEngine } from "../src/audio-engine.js";

const SAMPLES_PER_MS = 48;
const QUANTUM = 128;
const BUDGET_MS = 1_000;
const RAMP_FRAMES = 240;
const CHUNK_FRAMES = 12_000;

type ChunkRequest = { chunkFrames: number; generation: number; startSample: number };
type Sent = Record<string, unknown> & { type: string };

/** Each sample says which timeline position it is, so stale audio can't pass for current. */
const sampleValue = (position: number) => position / 1_000_000;

/** The real engine and the real worklet, on a clock and a mixer the test controls. */
async function createAudio() {
  const state = {
    now: 0,
    plays: [] as Sent[],
    requests: [] as (ChunkRequest & { at: number })[],
    resume: () => Promise.resolve() as Promise<void>,
    worker: null as { onmessage: ((event: { data: unknown }) => void) | null } | null,
  };
  vi.spyOn(performance, "now").mockImplementation(() => state.now);
  vi.stubGlobal("location", { origin: "https://app.test" });
  vi.stubGlobal("origin", "https://app.test");
  vi.stubGlobal("document", { baseURI: "https://app.test/" });
  vi.stubGlobal(
    "AudioWorkletProcessor",
    class {
      port = { onmessage: null, postMessage: (_message: unknown) => {} };
    },
  );
  vi.stubGlobal("registerProcessor", () => {});
  vi.stubGlobal("currentTime", 0);
  vi.resetModules();
  const { LumenAudioWorkletProcessor } = await import("../src/audio-worklet.js");
  const { LumenAudioEngine: Engine } = await import("../src/audio-engine.js");
  const processor = new LumenAudioWorkletProcessor();

  vi.stubGlobal(
    "AudioContext",
    class {
      audioWorklet = { addModule: () => Promise.resolve() };
      destination = {};
      onstatechange = null;
      state = "running";
      close = async () => {};
      createGain = () => ({
        connect() {},
        disconnect() {},
        gain: { setTargetAtTime() {}, value: 1 },
      });
      resume = () => state.resume();
      get currentTime() {
        return state.now / 1_000;
      }
    },
  );
  vi.stubGlobal(
    "AudioWorkletNode",
    class {
      port = {
        onmessage: null as ((event: { data: unknown }) => void) | null,
        postMessage: (message: Sent) => {
          if (message.type === "play") {
            state.plays.push(message);
          }
          processor.handleMessage(message);
        },
      };
      connect() {}
      disconnect() {}
      constructor() {
        processor.port.postMessage = (message: unknown) => this.port.onmessage?.({ data: message });
      }
    },
  );
  vi.stubGlobal(
    "Worker",
    class {
      onerror: unknown = null;
      onmessage: ((event: { data: unknown }) => void) | null = null;
      constructor() {
        state.worker = this;
      }
      postMessage(message: Sent & ChunkRequest) {
        if (message.type === "request-chunk") {
          state.requests.push({ ...message, at: state.now });
        }
      }
      terminate() {}
    },
  );

  const engine: LumenAudioEngine = new Engine({
    workerModuleUrl: "mixer.js",
    workletModuleUrl: "worklet.js",
  });
  engine.loadAudioTimeline(
    JSON.stringify({
      durationFrames: 18_000,
      fps: 30,
      segments: [
        {
          frames: 28_800_000,
          from: 0,
          gain: [1, 1],
          position: 0,
          start: 0,
          step: 1,
          to: 28_800_000,
          voice: 0,
        },
      ],
      voices: [{ duration: 28_800_000, fadeIn: 0, fadeOut: 0, sourceId: "voice", sourceStart: 0 }],
    }),
  );

  const deliver = ({ chunkFrames, generation, startSample }: ChunkRequest) =>
    state.worker?.onmessage?.({
      data: {
        channelCount: 2,
        frames: chunkFrames,
        generation,
        sampleRate: 48_000,
        samples: Float32Array.from({ length: chunkFrames * 2 }, (_, index) =>
          sampleValue(startSample + Math.floor(index / 2)),
        ),
        startSample,
        type: "chunk",
      },
    });

  return {
    deliver,
    engine,
    processor,
    state,
    /** Runs the audio thread for one quantum, returning where it read from and what it output. */
    renderQuantum() {
      state.now += QUANTUM / SAMPLES_PER_MS;
      vi.stubGlobal("currentTime", state.now / 1_000);
      const position = processor.currentSample as number;
      const left = new Float32Array(QUANTUM);
      processor.process([], [[left, new Float32Array(QUANTUM)]]);
      return { left, position };
    },
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve));

describe("audio that starts late", () => {
  let audio: Awaited<ReturnType<typeof createAudio>>;

  beforeEach(async () => {
    audio = await createAudio();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  /** Plays from `fromMs`, and returns the position the clock should be at once it's running. */
  async function playFrom(fromMs: number) {
    const startedAt = audio.state.now;
    audio.engine.play(fromMs);
    await flush();
    return () => fromMs + Math.max(audio.state.now - startedAt - BUDGET_MS, 0);
  }

  it("holds for its budget, then runs, with a mixer that never answers", async () => {
    const expected = await playFrom(200);

    while (audio.state.now < BUDGET_MS - 10) {
      audio.renderQuantum();
      expect(audio.engine.isClockRunning()).toBe(false);
      expect(audio.engine.currentTimeMs()).toBe(200);
    }
    while (audio.state.now < BUDGET_MS + 10) {
      audio.renderQuantum();
    }
    expect(audio.engine.isClockRunning()).toBe(true);

    while (audio.state.now < 60_000) {
      const { left } = audio.renderQuantum();
      expect(left.every((sample) => sample === 0)).toBe(true);
    }
    expect(audio.engine.isClockRunning()).toBe(true);
    expect(Math.abs(audio.engine.currentTimeMs() - expected())).toBeLessThan(10);
  });

  it("doesn't restart or stall the clock for deliveries that keep arriving late", async () => {
    const LATENCY_MS = 1_500;
    const expected = await playFrom(0);
    let lastPosition = 0;

    while (audio.state.now < 8_000) {
      while (
        audio.state.requests[0] &&
        audio.state.now - audio.state.requests[0].at >= LATENCY_MS
      ) {
        audio.deliver(audio.state.requests.shift() as ChunkRequest & { at: number });
      }
      audio.renderQuantum();
      const position = audio.engine.currentTimeMs();
      expect(position).toBeGreaterThanOrEqual(lastPosition - 1);
      lastPosition = position;
      if (audio.state.now > BUDGET_MS + 10) {
        expect(audio.engine.isClockRunning()).toBe(true);
        expect(Math.abs(position - expected())).toBeLessThan(10);
      }
    }
    expect(audio.state.plays).toHaveLength(1);
  });

  it("never plays samples behind the clock, then joins at the right position", async () => {
    const expected = await playFrom(0);
    while (audio.state.now < 1_500) {
      audio.renderQuantum();
    }
    const clockSample = expected() * SAMPLES_PER_MS;

    const requested = audio.state.requests.splice(0);
    const behind = requested.filter((r) => r.startSample + r.chunkFrames <= clockSample);
    expect(behind.length).toBeGreaterThan(0);
    behind.forEach(audio.deliver);
    for (let quantum = 0; quantum < 20; quantum += 1) {
      const { left } = audio.renderQuantum();
      expect(left.every((sample) => sample === 0)).toBe(true);
    }

    requested.filter((r) => !behind.includes(r)).forEach(audio.deliver);
    let heard = 0;
    for (let quantum = 0; quantum < 40; quantum += 1) {
      const { left, position } = audio.renderQuantum();
      expect(Math.abs(position / SAMPLES_PER_MS - expected())).toBeLessThan(10);
      left.forEach((sample, index) => {
        if (sample !== 0) {
          heard += 1;
          if (heard > RAMP_FRAMES) {
            expect(sample).toBeCloseTo(sampleValue(position + index), 5);
          } else {
            expect(sample).toBeLessThanOrEqual(sampleValue(position + index));
          }
        }
      });
    }
    expect(heard).toBeGreaterThan(RAMP_FRAMES);
    expect(audio.state.plays).toHaveLength(1);
  });

  it("doesn't stall again when the audio drops out after a late start", async () => {
    const expected = await playFrom(0);
    while (audio.state.now < 1_500) {
      audio.renderQuantum();
    }
    audio.state.requests.splice(0).forEach(audio.deliver);
    audio.state.requests.splice(0);

    while (audio.state.now < 6_000) {
      audio.renderQuantum();
      expect(audio.engine.isClockRunning()).toBe(true);
    }
    expect(Math.abs(audio.engine.currentTimeMs() - expected())).toBeLessThan(10);
    expect(audio.state.plays).toHaveLength(1);
  });

  it("starts at the elapsed position with no hold left when the context resumes late", async () => {
    let resumed = () => {};
    audio.state.resume = () =>
      new Promise((resolve) => {
        resumed = resolve;
      });
    audio.engine.play(100);
    await flush();
    expect(audio.state.plays).toHaveLength(0);

    audio.state.now = 700;
    expect(audio.engine.isClockRunning()).toBe(false);
    expect(audio.engine.currentTimeMs()).toBe(100);

    audio.state.now = 3_000;
    vi.stubGlobal("currentTime", 3);
    expect(audio.engine.isClockRunning()).toBe(true);
    expect(audio.engine.currentTimeMs()).toBe(2_100);

    resumed();
    await flush();
    expect(audio.state.plays[0]).toMatchObject({
      holdFrames: 0,
      startSample: 2_100 * SAMPLES_PER_MS,
    });
    expect(audio.engine.isClockRunning()).toBe(true);
    expect(audio.engine.currentTimeMs()).toBe(2_100);
  });

  it("still leads with audio that arrives within the budget, holding through an underrun", async () => {
    await playFrom(300);
    const first = audio.state.requests.splice(0);
    while (audio.state.now < 400) {
      audio.renderQuantum();
      expect(audio.engine.currentTimeMs()).toBe(300);
    }

    audio.deliver(first[0] as ChunkRequest);
    const { left, position } = audio.renderQuantum();
    expect(position).toBe(300 * SAMPLES_PER_MS);
    expect(left[0]).toBeCloseTo(sampleValue(position), 5);
    expect(audio.engine.isClockRunning()).toBe(true);

    while (audio.engine.isClockRunning()) {
      audio.renderQuantum();
    }
    const heldAt = audio.engine.currentTimeMs();
    expect(audio.processor.currentSample).toBeGreaterThan(first[0]?.startSample ?? 0);
    expect(audio.processor.currentSample).toBe((first[0]?.startSample ?? 0) + CHUNK_FRAMES);
    for (let quantum = 0; quantum < 1_000; quantum += 1) {
      audio.renderQuantum();
    }
    expect(audio.engine.currentTimeMs()).toBe(heldAt);
    expect(audio.engine.isClockRunning()).toBe(false);
  });
});
