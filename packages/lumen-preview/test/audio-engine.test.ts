import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AUDIO_STALL_TIMEOUT_MS, LumenAudioEngine } from "../src/audio-engine.js";
import { LUMEN_AUDIO_SAMPLE_RATE, type AudioEngineTimeline } from "../src/audio-protocol.js";
import { errorMediaFailure } from "../src/media/errors.js";

type Posted = {
  generation?: number;
  holdFrames?: number;
  id?: string;
  rate?: number;
  range?: unknown;
  startSample?: number;
  type: string;
};

type FakeGain = {
  connected: boolean;
  gain: { setTargetAtTime: ReturnType<typeof vi.fn>; value: number };
};

type Fakes = ReturnType<typeof installFakes>;

function installFakes() {
  const fakes = {
    addModule: (): Promise<void> => Promise.resolve(),
    contexts: [] as {
      closed: boolean;
      currentTime: number;
      onstatechange: (() => void) | null;
      state: string;
    }[],
    decoded: [] as number[],
    gains: [] as FakeGain[],
    decodeError: null as Error | null,
    decodeGates: new Map<number, Promise<void>>(),
    nodes: [] as { connected: boolean; onmessage: (message: unknown) => void; posted: Posted[] }[],
    workers: [] as {
      onmessage: (message: unknown) => void;
      posted: Posted[];
      terminated: boolean;
    }[],
  };

  vi.stubGlobal("location", { origin: "https://app.test" });

  vi.stubGlobal("origin", "https://app.test");
  vi.stubGlobal("document", { baseURI: "https://app.test/" });
  vi.stubGlobal(
    "AudioContext",
    class {
      audioWorklet = { addModule: () => fakes.addModule() };
      closed = false;
      currentTime = 0;
      destination = {};
      onstatechange: (() => void) | null = null;
      state = "running";
      constructor() {
        fakes.contexts.push(this);
      }
      close = async () => {
        this.closed = true;
      };
      createGain = () => {
        const gain: FakeGain & { connect(): void; disconnect(): void } = {
          connect: () => {
            gain.connected = true;
          },
          connected: false,
          disconnect: () => {
            gain.connected = false;
          },
          gain: { setTargetAtTime: vi.fn(), value: 1 },
        };
        fakes.gains.push(gain);
        return gain;
      };
      resume = async () => {};
    },
  );
  vi.stubGlobal(
    "AudioWorkletNode",
    class {
      connected = false;
      posted: Posted[] = [];
      port = {
        onmessage: null as ((event: { data: unknown }) => void) | null,
        postMessage: (message: Posted) => this.posted.push(message),
      };
      onmessage = (data: unknown) => this.port.onmessage?.({ data });
      constructor() {
        fakes.nodes.push(this);
      }
      connect() {
        this.connected = true;
      }
      disconnect() {
        this.connected = false;
      }
    },
  );
  vi.stubGlobal(
    "OfflineAudioContext",
    class {
      decodeAudioData = async (bytes: ArrayBuffer) => {
        const marker = new Uint8Array(bytes)[0] ?? 0;
        await fakes.decodeGates.get(marker);
        if (marker === 0) {
          throw new DOMException("Unable to decode audio data", "EncodingError");
        }
        fakes.decoded.push(marker);
        const channel = new Float32Array(4).fill(marker / 10);
        return {
          length: channel.length,
          numberOfChannels: 2,
          sampleRate: LUMEN_AUDIO_SAMPLE_RATE,
          getChannelData: () => channel,
        };
      };
    },
  );
  vi.stubGlobal(
    "Worker",
    class {
      onerror: unknown = null;
      onmessage: unknown = null;
      posted: Posted[] = [];
      terminated = false;
      constructor() {
        fakes.workers.push(this as never);
      }
      postMessage(message: Posted) {
        this.posted.push(message);
      }
      terminate() {
        this.terminated = true;
      }
    },
  );
  return fakes;
}

function createEngine(): LumenAudioEngine {
  return new LumenAudioEngine({ workerModuleUrl: "worker.js", workletModuleUrl: "worklet.js" });
}

function timelineJson(...sourceIds: string[]): string {
  const timeline: AudioEngineTimeline = {
    durationFrames: 240,
    fps: 24,
    segments: sourceIds.map((_, voice) => ({
      frames: 48_000,
      from: 0,
      gain: [1, 1],
      position: 0,
      start: 0,
      step: 1,
      to: 48_000,
      voice,
    })),
    voices: sourceIds.map((sourceId) => ({
      duration: 48_000,
      fadeIn: 0,
      fadeOut: 0,
      sourceId,
      sourceStart: 0,
    })),
  };
  return JSON.stringify(timeline);
}

function postedOfType(posted: readonly Posted[] | undefined, type: string): Posted[] {
  return (posted ?? []).filter((message) => message.type === type);
}

async function settled(fakes: Fakes): Promise<void> {
  await vi.waitFor(() => expect(fakes.nodes.length).toBeGreaterThan(0));
  await vi.waitFor(() =>
    expect(postedOfType(fakes.nodes[0]?.posted, "play").length).toBeGreaterThan(0),
  );
}

describe("LumenAudioEngine", () => {
  let fakes: Fakes;

  beforeEach(() => {
    fakes = installFakes();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("touches no Web Audio or worker for a composition without clips", async () => {
    const engine = createEngine();

    for (const json of [null, timelineJson()]) {
      engine.loadAudioTimeline(json);
      engine.play(0);
    }

    expect(engine.isPlaying()).toBe(false);
    expect(fakes.contexts).toEqual([]);
    expect(fakes.workers).toEqual([]);
  });

  it("never starts when paused while the worklet is loading", async () => {
    let finishLoading = () => {};
    fakes.addModule = () => new Promise((resolve) => (finishLoading = resolve));
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));

    engine.play(500);
    engine.pause();
    finishLoading();
    await vi.waitFor(() => expect(fakes.nodes).toHaveLength(1));
    await Promise.resolve();

    expect(engine.isPlaying()).toBe(false);
    expect(engine.currentTimeMs()).toBe(500);
    expect(postedOfType(fakes.nodes[0]?.posted, "play")).toEqual([]);
  });

  it("creates nothing when disposed while the worklet is loading", async () => {
    let finishLoading = () => {};
    fakes.addModule = () => new Promise((resolve) => (finishLoading = resolve));
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));

    engine.play(0);
    engine.dispose();
    finishLoading();
    await new Promise((resolve) => setTimeout(resolve));

    expect(fakes.nodes).toEqual([]);
    expect(fakes.contexts).toHaveLength(1);
    expect(fakes.contexts[0]?.closed).toBe(true);
    expect(fakes.workers).toEqual([]);

    engine.play(0);
    await engine.loadSource("a", new Uint8Array([1]));
    expect(engine.isPlaying()).toBe(false);
    expect(fakes.contexts).toHaveLength(1);
    expect(fakes.decoded).toEqual([]);
  });

  it("retries the worklet after it fails to load", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    fakes.addModule = () => Promise.reject(new Error("blocked"));
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));

    engine.play(0);
    await vi.waitFor(() => expect(error).toHaveBeenCalledOnce());
    expect(engine.isPlaying()).toBe(false);

    fakes.addModule = () => Promise.resolve();
    engine.play(0);
    await settled(fakes);
    expect(engine.isPlaying()).toBe(true);
  });

  it("replaces a source's samples with each load, and drops them when it is removed", async () => {
    vi.stubGlobal("fetch", async () => new Response(new Uint8Array([3])));
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));

    await engine.loadSource("a", new Uint8Array([1]));
    await engine.loadSource("a", new Uint8Array([2]));
    await engine.loadSource("a", { kind: "url", url: "https://a.test/song.wav" });
    expect(fakes.decoded).toEqual([1, 2, 3]);
    expect(postedOfType(fakes.workers[0]?.posted, "set-source")).toHaveLength(3);

    engine.removeSource("a");
    engine.removeSource("a");
    expect(postedOfType(fakes.workers[0]?.posted, "remove-source")).toEqual([
      { id: "a", type: "remove-source" },
    ]);
  });

  it("mixes again from the playhead when a source changes during playback", async () => {
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));
    engine.play(500);
    await settled(fakes);
    const requests = postedOfType(fakes.workers[0]?.posted, "request-chunk").length;

    await engine.loadSource("a", new Uint8Array([1]));
    await vi.waitFor(() => expect(postedOfType(fakes.nodes[0]?.posted, "play")).toHaveLength(2));

    expect(postedOfType(fakes.nodes[0]?.posted, "play")[1]?.startSample).toBe(24_000);
    expect(postedOfType(fakes.workers[0]?.posted, "request-chunk").length).toBeGreaterThan(
      requests,
    );
  });

  it("fails a source it can't fetch or decode as media, and loads it when tried again", async () => {
    vi.stubGlobal("fetch", async () => new Response(null, { status: 503 }));
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("offline", "corrupt", "music"));
    const offline = { kind: "url", url: "https://a.test/offline.wav" } as const;
    const failure = (load: Promise<void>) =>
      load.then(
        () => null,
        (error: unknown) => errorMediaFailure(error),
      );

    expect(await failure(engine.loadSource("offline", offline))).toMatchObject({
      code: "media.not_found",
      source: "offline",
      wanted: ["audio"],
    });
    expect(await failure(engine.loadSource("corrupt", new Uint8Array([0])))).toMatchObject({
      code: "media.decode",
      source: "corrupt",
    });
    await engine.loadSource("music", new Uint8Array([1]));
    expect(fakes.decoded).toEqual([1]);

    vi.stubGlobal("fetch", async () => new Response(new Uint8Array([2])));
    await engine.loadSource("offline", offline);
    expect(fakes.decoded).toEqual([1, 2]);
  });

  it("stops mixing the old audio when a replacement fails to load", async () => {
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));
    await engine.loadSource("a", new Uint8Array([1]));

    await expect(engine.loadSource("a", new Uint8Array([0]))).rejects.toThrow('audio source "a"');

    expect(postedOfType(fakes.workers[0]?.posted, "remove-source")).toHaveLength(1);
  });

  describe("when a source is replaced or removed while loading", () => {
    function gate(marker: number) {
      let release = () => {};
      fakes.decodeGates.set(marker, new Promise((resolve) => (release = resolve)));
      return release;
    }
    const setSources = () => postedOfType(fakes.workers[0]?.posted, "set-source");
    const plays = () => postedOfType(fakes.nodes[0]?.posted, "play");

    it("publishes only the latest source, whichever decode finishes first", async () => {
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));
      engine.play(0);
      await settled(fakes);
      const releaseOld = gate(1);
      const releaseNew = gate(2);

      const obsolete = engine.loadSource("a", new Uint8Array([1]));
      const current = engine.loadSource("a", new Uint8Array([2]));
      await vi.waitFor(() => expect(fakes.decodeGates.size).toBe(2));
      const playsBefore = plays().length;

      releaseOld();
      await obsolete;
      await new Promise((resolve) => setTimeout(resolve));
      expect(setSources()).toEqual([]);
      expect(plays()).toHaveLength(playsBefore);

      releaseNew();
      await current;
      expect(setSources()).toHaveLength(1);
      expect(plays()).toHaveLength(playsBefore + 1);
    });

    it("aborts a superseded fetch and doesn't wait for it", async () => {
      let signal: AbortSignal | undefined;
      vi.stubGlobal("fetch", (_url: string, init: RequestInit) => {
        signal = init.signal ?? undefined;
        return new Promise(() => {});
      });
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));

      void engine.loadSource("a", "https://a.test/hung.wav");
      await vi.waitFor(() => expect(signal).toBeDefined());
      expect(signal?.aborted).toBe(false);

      await engine.loadSource("a", new Uint8Array([2]));

      expect(signal?.aborted).toBe(true);
      expect(fakes.decoded).toEqual([2]);
      expect(setSources()).toHaveLength(1);
    });

    it("keeps the abort signal on a caller's request working", async () => {
      let signal: AbortSignal | undefined;
      vi.stubGlobal("fetch", (_url: string, init: RequestInit) => {
        signal = init.signal ?? undefined;
        return new Promise(() => {});
      });
      const own = new AbortController();
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));

      void engine.loadSource("a", new Request("https://a.test/hung.wav", { signal: own.signal }));
      await vi.waitFor(() => expect(signal).toBeDefined());
      own.abort();

      expect(signal?.aborted).toBe(true);
    });

    it("ignores the failure of a superseded source", async () => {
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));
      const releaseBad = gate(0);

      const obsolete = engine.loadSource("a", new Uint8Array([0]));
      await engine.loadSource("a", new Uint8Array([2]));
      releaseBad();

      await expect(obsolete).resolves.toBeUndefined();
      expect(postedOfType(fakes.workers[0]?.posted, "remove-source")).toEqual([]);
      expect(setSources()).toHaveLength(1);
    });

    it("resolves a load still fetching once the engine is disposed", async () => {
      vi.stubGlobal(
        "fetch",
        (_url: string, init: RequestInit) =>
          new Promise((_resolve, reject) => {
            init.signal?.addEventListener("abort", () =>
              reject(new DOMException("Aborted", "AbortError")),
            );
          }),
      );
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("stalled"));
      const stalled = engine.loadSource("stalled", "https://a.test/stalled.wav");
      await new Promise((resolve) => setTimeout(resolve));

      engine.dispose();

      await expect(stalled).resolves.toBeUndefined();
    });

    it("publishes nothing for a source removed before its decode finished", async () => {
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));
      const release = gate(1);

      const obsolete = engine.loadSource("a", new Uint8Array([1]));
      engine.removeSource("a");
      release();
      await obsolete;

      expect(setSources()).toEqual([]);
      expect(postedOfType(fakes.workers[0]?.posted, "remove-source")).toEqual([]);
    });
  });

  it("keeps playing through an identical timeline, and stops and parks at 0 for a new one", async () => {
    const engine = createEngine();
    const json = timelineJson("a");
    expect(engine.loadAudioTimeline(json)).toBe(true);
    engine.play(500);
    await settled(fakes);
    const worker = fakes.workers[0];
    const node = fakes.nodes[0];
    expect(postedOfType(worker?.posted, "set-timeline")).toHaveLength(1);
    expect(postedOfType(node?.posted, "pause")).toHaveLength(0);

    expect(engine.loadAudioTimeline(json)).toBe(false);
    expect(engine.isPlaying()).toBe(true);
    expect(postedOfType(node?.posted, "pause")).toHaveLength(0);
    expect(postedOfType(worker?.posted, "set-timeline")).toHaveLength(1);

    expect(engine.loadAudioTimeline(timelineJson("a", "b"))).toBe(true);
    expect(engine.isPlaying()).toBe(false);
    expect(engine.currentTimeMs()).toBe(0);
    expect(postedOfType(worker?.posted, "set-timeline")).toHaveLength(2);
  });

  it("follows the worklet's position, holding through a stall", async () => {
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));
    engine.play(500);
    const context = fakes.contexts[0];
    if (!context) throw new Error("no context");
    await settled(fakes);
    const generation = postedOfType(fakes.nodes[0]?.posted, "play")[0]?.generation;
    const report = (contextTime: number, seconds: number, stalled: boolean) =>
      fakes.nodes[0]?.onmessage({
        contextTime,
        currentSample: seconds * LUMEN_AUDIO_SAMPLE_RATE,
        generation,
        rateRevision: 0,
        stalled,
        type: "position",
      });

    context.currentTime = 5;
    expect(engine.currentTimeMs()).toBe(500);

    report(5, 0.5, false);
    context.currentTime = 5.25;
    expect(engine.currentTimeMs()).toBeCloseTo(750);

    report(5.25, 0.75, true);
    context.currentTime = 7;
    expect(engine.currentTimeMs()).toBeCloseTo(750);

    report(7, 0.75, false);
    context.currentTime = 7.1;
    expect(engine.currentTimeMs()).toBeCloseTo(850);
  });

  it("reports whether the position is moving, and calls out when that changes", async () => {
    const onClockChange = vi.fn();
    const engine = new LumenAudioEngine({
      onClockChange,
      workerModuleUrl: "worker.js",
      workletModuleUrl: "worklet.js",
    });
    engine.loadAudioTimeline(timelineJson("a"));
    engine.play(0);
    await settled(fakes);
    const context = fakes.contexts[0];
    if (!context) throw new Error("no context");
    const generation = postedOfType(fakes.nodes[0]?.posted, "play")[0]?.generation;
    const report = (stalled: boolean) =>
      fakes.nodes[0]?.onmessage({
        contextTime: context.currentTime,
        currentSample: 0,
        generation,
        rateRevision: 0,
        stalled,
        type: "position",
      });
    expect(engine.isClockRunning()).toBe(false);

    report(false);
    expect(engine.isClockRunning()).toBe(true);
    expect(onClockChange).toHaveBeenCalledTimes(1);

    report(true);
    expect(engine.isClockRunning()).toBe(false);
    expect(onClockChange).toHaveBeenCalledTimes(2);

    report(false);
    context.state = "suspended";
    context.onstatechange?.();
    expect(engine.isClockRunning()).toBe(false);
    expect(onClockChange).toHaveBeenCalledTimes(4);
  });

  describe("when audio stops arriving", () => {
    async function playStalled(onError: () => void, onClockChange = vi.fn()) {
      const engine = new LumenAudioEngine({
        onClockChange,
        onError,
        workerModuleUrl: "worker.js",
        workletModuleUrl: "worklet.js",
      });
      engine.loadAudioTimeline(timelineJson("a"));
      engine.play(0);
      await settled(fakes);
      const generation = postedOfType(fakes.nodes[0]?.posted, "play")[0]?.generation;
      const report = (stalled: boolean) =>
        fakes.nodes[0]?.onmessage({
          contextTime: 0,
          currentSample: 0,
          generation,
          rateRevision: 0,
          stalled,
          type: "position",
        });
      return { engine, onClockChange, report };
    }

    beforeEach(() => {
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance"] });
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it("reports once after the timeout and moves the position without audio", async () => {
      const onError = vi.fn();
      const { engine, onClockChange, report } = await playStalled(onError);
      report(false);
      report(true);
      report(true);
      expect(engine.isClockRunning()).toBe(false);

      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS - 1);
      expect(onError).not.toHaveBeenCalled();
      vi.advanceTimersByTime(1);
      expect(onError).toHaveBeenCalledTimes(1);
      expect(onClockChange).toHaveBeenCalledTimes(3);
      expect(engine.isPlaying()).toBe(true);
      expect(engine.isClockRunning()).toBe(true);

      const before = engine.currentTimeMs();
      vi.advanceTimersByTime(500);
      expect(engine.currentTimeMs()).toBeCloseTo(before + 500);

      report(false);
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS * 2);
      expect(onError).toHaveBeenCalledTimes(1);
      expect(engine.isClockRunning()).toBe(true);
    });

    it("reports nothing when the underrun recovers inside the timeout", async () => {
      const onError = vi.fn();
      const { engine, report } = await playStalled(onError);
      report(true);
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS - 1);
      report(false);
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS * 2);

      expect(onError).not.toHaveBeenCalled();
      expect(engine.isClockRunning()).toBe(true);
    });

    it("reports nothing when playback pauses during the stall", async () => {
      const onError = vi.fn();
      const { engine, report } = await playStalled(onError);
      report(true);
      engine.pause();
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS * 2);

      expect(onError).not.toHaveBeenCalled();
    });

    it("does not count a suspended context as a stall", async () => {
      const onError = vi.fn();
      const { report } = await playStalled(onError);
      report(true);
      const context = fakes.contexts[0];
      if (!context) throw new Error("no context");
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS - 1);
      context.state = "suspended";
      context.onstatechange?.();
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS * 2);
      expect(onError).not.toHaveBeenCalled();

      context.state = "running";
      context.onstatechange?.();
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS - 1);
      expect(onError).not.toHaveBeenCalled();
      vi.advanceTimersByTime(1);
      expect(onError).toHaveBeenCalledTimes(1);
    });

    it("keeps audio off when a source arrives after giving up, until the next play", async () => {
      const onError = vi.fn();
      const { engine, report } = await playStalled(onError);
      const plays = () => postedOfType(fakes.nodes[0]?.posted, "play");
      report(true);
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS);
      expect(plays()).toHaveLength(1);

      await engine.loadSource("a", new Uint8Array([1]));
      expect(plays()).toHaveLength(1);

      engine.pause();
      engine.play();
      await vi.waitFor(() => expect(plays()).toHaveLength(2));
    });

    it("keeps the position when the range changes after giving up", async () => {
      const { engine, report } = await playStalled(vi.fn());
      report(true);
      vi.advanceTimersByTime(AUDIO_STALL_TIMEOUT_MS);

      engine.setRange({ endMs: 10_000, loop: true, startMs: 0 });
      vi.advanceTimersByTime(25_000);
      expect(engine.currentTimeMs()).toBeCloseTo(5_000);

      engine.setRange({ endMs: 6_000, loop: true, startMs: 0 });
      expect(engine.currentTimeMs()).toBeCloseTo(5_000);
      vi.advanceTimersByTime(2_000);
      expect(engine.currentTimeMs()).toBeCloseTo(1_000);
    });
  });

  it("keeps a spent budget when a source change restarts the mix", async () => {
    const now = vi.spyOn(performance, "now").mockReturnValue(1_000);
    const onClockChange = vi.fn();
    const engine = new LumenAudioEngine({
      onClockChange,
      workerModuleUrl: "worker.js",
      workletModuleUrl: "worklet.js",
    });
    engine.loadAudioTimeline(timelineJson("a"));
    await engine.loadSource("a", new Uint8Array([1]));
    engine.play(0);
    await settled(fakes);
    const plays = () => postedOfType(fakes.nodes[0]?.posted, "play");
    expect(plays()[0]?.holdFrames).toBe(LUMEN_AUDIO_SAMPLE_RATE);

    now.mockReturnValue(5_000);
    await engine.loadSource("a", new Uint8Array([2]));
    await vi.waitFor(() => expect(plays()).toHaveLength(2));
    expect(plays()[1]?.holdFrames).toBe(0);
    expect(engine.isClockRunning()).toBe(true);
    now.mockRestore();
  });

  it("drops worker chunks and worklet reports from before a pause", async () => {
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));
    engine.play(0);
    await settled(fakes);
    const generation = postedOfType(fakes.nodes[0]?.posted, "play")[0]?.generation;

    engine.pause();
    fakes.workers[0]?.onmessage({
      data: {
        generation,
        samples: new Float32Array(2),
        startSample: 0,
        type: "chunk",
      },
    });
    fakes.nodes[0]?.onmessage({
      contextTime: 1,
      currentSample: 48_000,
      generation,
      rateRevision: 0,
      stalled: false,
      type: "position",
    });

    expect(postedOfType(fakes.nodes[0]?.posted, "chunk")).toEqual([]);
    expect(engine.currentTimeMs()).toBe(0);
  });

  it("requests each chunk once as the playhead moves", async () => {
    const engine = createEngine();
    engine.loadAudioTimeline(timelineJson("a"));
    engine.play(0);
    await settled(fakes);
    const generation = postedOfType(fakes.nodes[0]?.posted, "play")[0]?.generation;
    const requested = () =>
      postedOfType(fakes.workers[0]?.posted, "request-chunk").map((message) => message.startSample);
    const before = requested();
    expect(before.at(0)).toBe(0);
    expect(before.at(-1)).toBe(96_000);

    fakes.nodes[0]?.onmessage({
      contextTime: 1,
      currentSample: 24_000,
      generation,
      rateRevision: 0,
      stalled: false,
      type: "position",
    });

    expect(requested().slice(before.length)).toEqual([108_000, 120_000]);
  });

  describe("rate, range and volume", () => {
    async function playing(fromMs = 0) {
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));
      return { engine, ...(await start(engine, fromMs)) };
    }

    async function start(engine: LumenAudioEngine, fromMs: number) {
      engine.play(fromMs);
      await settled(fakes);
      const node = fakes.nodes[0];
      const context = fakes.contexts[0];
      if (!node || !context) throw new Error("not playing");
      const generation = postedOfType(node.posted, "play")[0]?.generation;
      const report = (contextTime: number, seconds: number, rateRevision: number) =>
        node.onmessage({
          contextTime,
          currentSample: seconds * LUMEN_AUDIO_SAMPLE_RATE,
          generation,
          rateRevision,
          stalled: false,
          type: "position",
        });
      return { context, node, report };
    }

    it("starts at the rate it was given, and asks the mixer for the extra audio it needs", async () => {
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));
      engine.setRate(4);
      const { node } = await start(engine, 0);

      expect(postedOfType(node.posted, "play")[0]).toMatchObject({ rate: 4, rateRevision: 1 });
      const requested = postedOfType(fakes.workers[0]?.posted, "request-chunk").map(
        (message) => message.startSample,
      );
      expect(requested.at(-1)).toBe(384_000);
    });

    it("carries on from the same position at a new rate, ignoring reports from the old one", async () => {
      const { context, engine, node, report } = await playing();
      report(0, 0, 0);
      context.currentTime = 1;
      expect(engine.currentTimeMs()).toBeCloseTo(1_000);

      engine.setRate(2);
      expect(postedOfType(node.posted, "rate")).toEqual([
        { rate: 2, rateRevision: 1, type: "rate" },
      ]);
      expect(engine.currentTimeMs()).toBeCloseTo(1_000);
      context.currentTime = 2;
      expect(engine.currentTimeMs()).toBeCloseTo(3_000);

      report(1.5, 1.5, 0);
      expect(engine.currentTimeMs()).toBeCloseTo(3_000);
      report(2, 3, 1);
      context.currentTime = 2.5;
      expect(engine.currentTimeMs()).toBeCloseTo(4_000);
      expect(postedOfType(node.posted, "pause")).toHaveLength(0);
    });

    it("sends a rate change made before the first report, and follows the reports that answer it", async () => {
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));
      engine.play(0);
      await settled(fakes);
      const node = fakes.nodes[0];
      if (!node) throw new Error("not playing");
      const generation = postedOfType(node.posted, "play")[0]?.generation;
      const report = (rateRevision: number, seconds: number) =>
        node.onmessage({
          contextTime: 0,
          currentSample: seconds * LUMEN_AUDIO_SAMPLE_RATE,
          generation,
          rateRevision,
          stalled: false,
          type: "position",
        });

      engine.setRate(2);
      expect(postedOfType(node.posted, "rate")).toEqual([
        { rate: 2, rateRevision: 1, type: "rate" },
      ]);

      report(0, 0);
      expect(engine.isClockRunning()).toBe(false);
      report(1, 0.25);
      expect(engine.currentTimeMs()).toBeCloseTo(250);
      expect(engine.isClockRunning()).toBe(true);
    });

    it("tells apart reports from before a rate that returned to its old value", async () => {
      const { context, engine, report } = await playing();
      report(0, 0, 0);
      engine.setRate(2);
      engine.setRate(1);

      context.currentTime = 1;
      report(0.5, 5, 0);
      expect(engine.currentTimeMs()).not.toBeCloseTo(5_000);
      report(1, 1, 2);
      expect(engine.currentTimeMs()).toBeCloseTo(1_000);
    });

    it("wraps a looping range at its end and stays inside one that doesn't loop", async () => {
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));
      engine.setRange({ endMs: 4_000, loop: true, startMs: 2_000 });
      const { context, node, report } = await start(engine, 1_000);
      expect(engine.currentTimeMs()).toBe(3_000);
      expect(postedOfType(fakes.workers[0]?.posted, "set-range").at(-1)).toEqual({
        range: { endMs: 4_000, loop: true, startMs: 2_000 },
        type: "set-range",
      });

      report(5, 3.5, 1);
      context.currentTime = 6.5;
      expect(engine.currentTimeMs()).toBeCloseTo(3_000);

      engine.setRange({ endMs: 4_000, loop: false, startMs: 2_000 });
      await vi.waitFor(() => expect(postedOfType(node.posted, "play")).toHaveLength(2));
      expect(engine.currentTimeMs()).toBeCloseTo(3_000);
      engine.seekMs(9_000);
      expect(engine.currentTimeMs()).toBe(4_000);
      engine.seekMs(500);
      expect(engine.currentTimeMs()).toBe(2_000);
    });

    it("leaves playback alone when the range is the same", async () => {
      const engine = createEngine();
      engine.loadAudioTimeline(timelineJson("a"));
      engine.setRange({ endMs: 4_000, loop: true, startMs: 2_000 });
      const { node } = await start(engine, 2_500);

      engine.setRange({ endMs: 4_000, loop: true, startMs: 2_000 });
      expect(postedOfType(node.posted, "play")).toHaveLength(1);
    });

    it("sets the level without clicks, and mutes without losing the volume", async () => {
      const engine = createEngine();
      engine.setVolume(0.5, false);
      engine.loadAudioTimeline(timelineJson("a"));
      const { context } = await start(engine, 0);
      const gain = fakes.gains[0];
      if (!gain) throw new Error("no gain node");
      expect(gain.connected).toBe(true);
      expect(gain.gain.value).toBe(0.5);

      context.currentTime = 3;
      engine.setVolume(0.5, true);
      expect(gain.gain.setTargetAtTime).toHaveBeenLastCalledWith(0, 3, expect.any(Number));
      engine.setVolume(0.5, false);
      expect(gain.gain.setTargetAtTime).toHaveBeenLastCalledWith(0.5, 3, expect.any(Number));
      engine.setVolume(0.25, false);
      expect(gain.gain.setTargetAtTime).toHaveBeenLastCalledWith(0.25, 3, expect.any(Number));
      expect(gain.gain.value).toBe(0.5);

      engine.dispose();
      expect(gain.connected).toBe(false);
    });

    it("starts muted when asked to, before any audio exists", async () => {
      const engine = createEngine();
      engine.setVolume(0.8, true);
      engine.loadAudioTimeline(timelineJson("a"));
      await start(engine, 0);
      expect(fakes.gains[0]?.gain.value).toBe(0);
    });
  });
});
