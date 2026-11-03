import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LumenPreviewContext } from "../../src/preview.js";
import type { MediaRegistration } from "../../src/media/index.js";
import { LumenPreviewSession } from "../../src/session/session.js";

const SAMPLES_PER_MS = 48;
const QUANTUM = 128;
const TOLERANCE_SAMPLES = QUANTUM * 2;
const COMPOSITION_FPS = 30;

type Message = Record<string, unknown> & { type: string };
type ChunkRequest = { chunkFrames: number; generation: number; startSample: number };
type Processor = {
  currentSample: number;
  handleMessage(message: unknown): void;
  playing: boolean;
  port: { postMessage: (message: unknown) => void };
  process(inputs: unknown[], outputs: Float32Array[][]): boolean;
};
type Browser = Awaited<ReturnType<typeof installBrowser>>;

function timelineJson(): string {
  return JSON.stringify({
    clips: [
      {
        durationMs: 10_000,
        id: "clip",
        sourceId: "voice",
        sourceStartMs: 0,
        startMs: 0,
        trackId: "track",
        volume: 1,
      },
    ],
    durationFrames: 300,
    fps: COMPOSITION_FPS,
    tracks: [{ id: "track", muted: false, solo: false, volume: 1 }],
  });
}

function deferred<T = void>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve));
}

/** Replaces only browser interfaces. The audio thread runs the real worklet processor. */
async function installBrowser() {
  const browser = {
    addModule: (): Promise<void> => Promise.resolve(),
    resume: (): Promise<void> => Promise.resolve(),
    audioWorkers: [] as { onmessage: ((event: { data: unknown }) => void) | null }[],
    animationFrames: [] as ((now: number) => void)[],
    animationFramesRequested: 0,
    loadedCompositions: [] as string[],
    mixerStarved: false,
    mixerMessages: [] as Message[],
    now: 0,
    pendingChunks: [] as ChunkRequest[],
    previewWorkers: [] as {
      onmessage: ((event: { data: unknown }) => void) | null;
      posted: Message[];
    }[],
    processors: [] as Processor[],
    targetTimes: [] as number[],
  };

  vi.spyOn(performance, "now").mockImplementation(() => browser.now);

  vi.stubGlobal(
    "AudioWorkletProcessor",
    class {
      port = { onmessage: null, postMessage: (_message: unknown) => {} };
    },
  );
  vi.stubGlobal("registerProcessor", () => {});
  vi.stubGlobal("currentTime", 0);
  vi.resetModules();
  const { LumenAudioWorkletProcessor } = await import("../../src/audio-worklet.js");

  vi.stubGlobal(
    "AudioContext",
    class {
      audioWorklet = { addModule: () => browser.addModule() };
      destination = {};
      close = async () => {};
      resume = () => browser.resume();
      get currentTime() {
        return browser.now / 1_000;
      }
    },
  );
  vi.stubGlobal(
    "AudioWorkletNode",
    class {
      port = {
        onmessage: null as ((event: { data: unknown }) => void) | null,
        postMessage: (message: unknown) => processor.handleMessage(message),
      };
      connect() {}
      disconnect() {}
      constructor() {
        browser.processors.push(processor);
        processor.port.postMessage = (message) => this.port.onmessage?.({ data: message });
      }
    },
  );
  const processor = new LumenAudioWorkletProcessor() as unknown as Processor;

  const answerChunk = (worker: (typeof browser.audioWorkers)[number], request: ChunkRequest) => {
    worker.onmessage?.({
      data: {
        channelCount: 2,
        frames: request.chunkFrames,
        generation: request.generation,
        sampleRate: 48_000,
        samples: new Float32Array(request.chunkFrames * 2).fill(0.5),
        startSample: request.startSample,
        type: "chunk",
      },
    });
  };
  vi.stubGlobal(
    "Worker",
    class {
      onerror: unknown = null;
      onmessage: ((event: { data: unknown }) => void) | null = null;
      posted: Message[] = [];
      private readonly isMixer: boolean;
      constructor(url: string) {
        this.isMixer = url === "mixer.js";
        if (this.isMixer) {
          browser.audioWorkers.push(this);
        } else {
          browser.previewWorkers.push(this);
        }
      }
      postMessage(message: Message & ChunkRequest) {
        this.posted.push(message);
        if (!this.isMixer) {
          return;
        }
        browser.mixerMessages.push(message);
        if (message.type === "request-chunk") {
          if (browser.mixerStarved) {
            browser.pendingChunks.push(message);
          } else {
            answerChunk(this, message);
          }
        }
      }
      terminate() {}
    },
  );
  vi.stubGlobal("requestAnimationFrame", (callback: (now: number) => void) => {
    browser.animationFramesRequested += 1;
    browser.animationFrames.push(callback);
    return browser.animationFramesRequested;
  });
  vi.stubGlobal("cancelAnimationFrame", () => {});

  return {
    get state() {
      return browser;
    },
    deliverStarvedChunks() {
      browser.mixerStarved = false;
      const worker = browser.audioWorkers[0];
      for (const request of browser.pendingChunks.splice(0)) {
        if (worker) answerChunk(worker, request);
      }
    },
    get processor() {
      return processor;
    },
    /** Advances wall time and renders the audio thread for the same span. */
    render(ms: number): Float32Array[] {
      const rendered: Float32Array[] = [];
      for (let samples = 0; samples < ms * SAMPLES_PER_MS; samples += QUANTUM) {
        browser.now += QUANTUM / SAMPLES_PER_MS;
        vi.stubGlobal("currentTime", browser.now / 1_000);
        const left = new Float32Array(QUANTUM);
        processor.process([], [[left, new Float32Array(QUANTUM)]]);
        rendered.push(left);
      }
      return rendered;
    },
    async runAnimationFrame() {
      for (const callback of browser.animationFrames.splice(0)) {
        callback(browser.now);
      }
      await flush();
    },
  };
}

function fakeController(browser: Browser) {
  return class {
    frame = 0;
    playing = false;
    audioTimeline() {
      return timelineJson();
    }
    clear() {}
    clearMedia() {}
    clearVideos() {}
    clearVideoSource() {}
    currentFrame() {
      return this.frame;
    }
    durationFrames() {
      return 300;
    }
    fps() {
      return COMPOSITION_FPS;
    }
    frameRequirements() {
      return '{"images":[],"videos":[]}';
    }
    frameRequirementsWindow() {
      return '{"images":[],"videos":[]}';
    }
    hasFont() {
      return false;
    }
    hasImage() {
      return false;
    }
    hasVideoFrame() {
      return false;
    }
    height() {
      return 1;
    }
    isPlaying() {
      return this.playing;
    }
    loadComposition(json: string) {
      browser.state.loadedCompositions.push(json);
      this.frame = 0;
      this.playing = false;
    }
    pause() {
      this.playing = false;
    }
    play() {
      this.playing = true;
    }
    removeFontFamily() {}
    removeImageSource() {}
    removeVideoSource() {}
    async renderNow() {}
    setFont() {}
    setFrame(frame: number) {
      this.frame = frame;
    }
    setImage() {}
    setLogLevel() {}
    setLookaheadCount() {}
    setVideoFrame() {}
    setVideoMetadata() {}
    targetFrameDurationMs() {
      return 1_000 / COMPOSITION_FPS;
    }
    targetFrameForTimeMs(ms: number) {
      browser.state.targetTimes.push(ms);
      return Math.floor((ms / 1_000) * COMPOSITION_FPS);
    }
    async tick() {
      return false;
    }
    width() {
      return 1;
    }
  };
}

const AUDIO = { workerModuleUrl: "mixer.js", workletModuleUrl: "worklet.js" };
const SOURCES = [{ id: "voice", kind: "audio" as const, source: new Uint8Array([1]) }];

function stubDecoding() {
  vi.stubGlobal(
    "OfflineAudioContext",
    class {
      decodeAudioData = async () => ({
        length: 4,
        numberOfChannels: 2,
        getChannelData: () => new Float32Array(4),
      });
    },
  );
}

/** A media load that doesn't finish until the returned function is called. */
function stalledImage(): { release: () => void; sources: MediaRegistration[] } {
  const response = deferred<Response>();
  vi.stubGlobal("fetch", () => response.promise);
  vi.stubGlobal("createImageBitmap", async () => ({ close: () => {}, height: 1, width: 1 }));
  vi.stubGlobal(
    "OffscreenCanvas",
    class {
      getContext() {
        return { drawImage: () => {}, getImageData: () => ({ data: new Uint8ClampedArray(4) }) };
      }
    },
  );
  return {
    release: () => response.resolve(new Response("image")),
    sources: [{ id: "logo", kind: "image", source: "https://a.test/logo.png" }],
  };
}

async function startMainDriver(browser: Browser, compositionJson = "first") {
  const preview = new LumenPreviewContext();
  const session = new LumenPreviewSession({
    audio: AUDIO,
    audioSources: SOURCES,
    bindings: {
      LumenMediaStore: class {},
      LumenPreviewController: fakeController(browser),
      LumenRenderer: class {},
    } as never,
    compositionJson,
    preview,
  });
  await session.attach({} as HTMLCanvasElement);
  await flush();
  return { preview, session };
}

async function startWorkerDriver(browser: Browser) {
  const preview = new LumenPreviewContext();
  const session = new LumenPreviewSession({
    audio: AUDIO,
    audioSources: SOURCES,
    bindings: {
      preview: async () => ({}),
      previewWorkerUrl: () => "preview-worker.js",
    } as never,
    compositionJson: "first",
    preview,
  });
  await session.attach({ transferControlToOffscreen: () => ({}) } as never);
  const worker = browser.state.previewWorkers[0];
  const answerLoad = (loadId: number) =>
    worker?.onmessage?.({ data: { loadId, timelineJson: timelineJson(), type: "audio-timeline" } });
  worker?.onmessage?.({
    data: { patch: { isLoaded: true, totalFrames: 300 }, type: "state" },
  });
  worker?.onmessage?.({
    data: { stats: { timelineFps: COMPOSITION_FPS }, type: "stats" },
  });
  answerLoad(0);
  await flush();
  return { answerLoad, preview, session };
}

function isSilent(rendered: readonly Float32Array[]): boolean {
  return rendered.every((quantum) => quantum.every((sample) => sample === 0));
}

describe("preview drivers and the audio clock", () => {
  let browser: Browser;

  beforeEach(async () => {
    browser = await installBrowser();
    stubDecoding();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  describe("worker driver, whose video runs on its own clock", () => {
    it("ramps up when the mixer recovers from running dry during playback", async () => {
      const { preview } = await startWorkerDriver(browser);
      preview.play();
      await flush();
      browser.render(100);

      browser.state.mixerStarved = true;
      expect(isSilent(browser.render(3_000).slice(-10))).toBe(true);
      browser.deliverStarvedChunks();
      const left = Float32Array.from(browser.render(20).flatMap((quantum) => [...quantum]));

      let largestStep = 0;
      for (let frame = 1; frame < left.length; frame += 1) {
        largestStep = Math.max(largestStep, Math.abs((left[frame] ?? 0) - (left[frame - 1] ?? 0)));
      }
      expect(left[0]).toBe(0);
      expect(left.at(-1)).toBe(0.5);
      expect(largestStep).toBeLessThan(0.01);
      expect(
        Math.abs(browser.processor.currentSample - browser.state.now * SAMPLES_PER_MS),
      ).toBeLessThan(TOLERANCE_SAMPLES);
    });

    it.each(["addModule", "resume"] as const)(
      "starts audio at the elapsed position after a slow %s",
      async (step) => {
        const slowLoad = deferred();
        browser.state[step] = () => slowLoad.promise;
        const { preview } = await startWorkerDriver(browser);

        preview.play();
        await flush();
        browser.state.now += 300;
        slowLoad.resolve();
        await flush();
        browser.render(100);

        expect(
          Math.abs(browser.processor.currentSample - browser.state.now * SAMPLES_PER_MS),
        ).toBeLessThan(TOLERANCE_SAMPLES);
      },
    );

    it("stays on elapsed time while the mixer is starved, then plays the right samples", async () => {
      browser.state.mixerStarved = true;
      const { preview } = await startWorkerDriver(browser);

      preview.play();
      await flush();
      expect(isSilent(browser.render(500))).toBe(true);
      expect(
        Math.abs(browser.processor.currentSample - browser.state.now * SAMPLES_PER_MS),
      ).toBeLessThan(TOLERANCE_SAMPLES);

      browser.deliverStarvedChunks();
      const resumed = browser.render(20);

      expect(resumed.at(-1)?.every((sample) => sample === 0.5)).toBe(true);
      expect(
        Math.abs(browser.processor.currentSample - browser.state.now * SAMPLES_PER_MS),
      ).toBeLessThan(TOLERANCE_SAMPLES);
    });

    it("keeps the elapsed position when a source arrives before audio has started", async () => {
      const slowLoad = deferred();
      browser.state.addModule = () => slowLoad.promise;
      const { preview, session } = await startWorkerDriver(browser);

      preview.play();
      browser.state.now += 300;
      session.update({
        audioSources: [...SOURCES, { id: "music", kind: "audio", source: new Uint8Array([2]) }],
      });
      await flush();
      slowLoad.resolve();
      await flush();
      browser.render(20);

      expect(
        Math.abs(browser.processor.currentSample - browser.state.now * SAMPLES_PER_MS),
      ).toBeLessThan(TOLERANCE_SAMPLES);
    });
  });

  describe("main-thread driver, whose video follows the audio", () => {
    it("holds the audio position and the video through a starved mixer", async () => {
      browser.state.mixerStarved = true;
      const { preview } = await startMainDriver(browser);

      preview.play();
      await flush();
      browser.render(500);
      await browser.runAnimationFrame();

      expect(browser.processor.currentSample).toBe(0);
      expect(browser.state.targetTimes.at(-1)).toBe(0);

      browser.deliverStarvedChunks();
      browser.render(100);
      await browser.runAnimationFrame();

      expect(browser.processor.currentSample).toBeGreaterThanOrEqual(100 * SAMPLES_PER_MS);
      expect(browser.processor.currentSample).toBeLessThan(100 * SAMPLES_PER_MS + QUANTUM);
      expect(browser.state.targetTimes.at(-1)).toBeGreaterThan(90);
      expect(browser.state.targetTimes.at(-1)).toBeLessThan(110);
    });

    it("starts no animation loop when disposed while attaching", async () => {
      const media = stalledImage();
      const preview = new LumenPreviewContext();
      const session = new LumenPreviewSession({
        audio: AUDIO,
        bindings: {
          LumenMediaStore: class {},
          LumenPreviewController: fakeController(browser),
          LumenRenderer: class {},
        } as never,
        compositionJson: "first",
        mediaSources: media.sources,
        preview,
      });

      const attaching = session.attach({} as HTMLCanvasElement);
      await flush();
      session.dispose();
      media.release();
      await attaching;

      expect(browser.state.animationFramesRequested).toBe(0);
    });
  });

  describe.each([
    ["composition", (json: string, _sources: MediaRegistration[]) => ({ compositionJson: json })],
    ["media", (_json: string, sources: MediaRegistration[]) => ({ mediaSources: sources })],
  ])("editing the %s during playback", (_name, edit) => {
    it("main driver: stops audio at once, stays stopped while loading, installs on completion", async () => {
      const { preview, session } = await startMainDriver(browser);
      preview.play();
      await flush();
      expect(isSilent(browser.render(100))).toBe(false);

      const media = stalledImage();
      session.update({ ...edit("second", media.sources), mediaSources: media.sources });

      expect(browser.processor.playing).toBe(false);
      expect(isSilent(browser.render(100))).toBe(true);
      preview.play();
      await flush();
      expect(browser.processor.playing).toBe(false);

      browser.state.mixerMessages.length = 0;
      media.release();
      await vi.waitFor(() => expect(browser.state.loadedCompositions).toHaveLength(2));
      await flush();
      expect(browser.state.mixerMessages.at(-1)?.type).toBe("set-timeline");

      preview.play();
      await flush();
      expect(browser.processor.playing).toBe(true);
      expect(browser.processor.currentSample).toBe(0);
    });

    it("worker driver: stops audio at once, stays stopped while loading, installs on completion", async () => {
      const { answerLoad, preview, session } = await startWorkerDriver(browser);
      preview.play();
      await flush();
      expect(isSilent(browser.render(100))).toBe(false);

      const media = stalledImage();
      session.update(edit("second", media.sources));

      expect(browser.processor.playing).toBe(false);
      expect(isSilent(browser.render(100))).toBe(true);
      preview.play();
      await flush();
      expect(browser.processor.playing).toBe(false);

      preview.pause();
      browser.state.mixerMessages.length = 0;
      answerLoad(1);
      expect(browser.state.mixerMessages.at(-1)?.type).toBe("set-timeline");

      preview.play();
      await flush();
      expect(browser.processor.playing).toBe(true);
      expect(browser.processor.currentSample).toBeLessThan(QUANTUM);
    });
  });
});
