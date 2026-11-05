import { sameMediaInput, sourceInputToBlob } from "./media/source.js";
import type { MediaSourceInput } from "./media/types.js";

export const LUMEN_AUDIO_SAMPLE_RATE = 48_000;
export const LUMEN_AUDIO_CHANNELS = 2;
const MS_PER_SECOND = 1_000;

export interface AudioEngineTrack {
  id: string;
  muted: boolean;
  solo: boolean;
  volume: number;
}

export interface AudioEngineClip {
  durationMs: number;
  id: string;
  sourceId: string;
  sourceStartMs: number;
  startMs: number;
  trackId: string;
  volume: number;
}

export interface AudioEngineTimeline {
  clips: AudioEngineClip[];
  durationFrames: number;
  fps: number;
  tracks: AudioEngineTrack[];
}

export interface AudioSourceRegistration {
  id: string;
  kind: "audio";
  source: MediaSourceInput;
}

export interface LumenAudioEngineOptions {
  latencyHint?: AudioContextLatencyCategory | number;
  /** Called when the position starts or stops moving, or jumps because the mix restarted. */
  onClockChange?: () => void;
  workerModuleUrl?: string | URL;
  workletModuleUrl?: string | URL;
}

// How long playback waits for audio that hasn't started, such as a page with no user gesture yet.
// After that the position runs on elapsed time, and the audio joins there if it ever starts.
const AUDIO_START_BUDGET_MS = 1_000;

/** The last position the worklet played, or the requested start until the worklet has started. */
type PlaybackClock = {
  contextTime: number;
  ms: number;
  stalled: boolean;
  startedAt: number;
  workletStarted: boolean;
};

// Async work holds on to its entry and publishes only while the map still holds that same object.
type AudioSourceEntry = {
  abort: AbortController;
  /** Resolves to a failure message, or null once loaded or superseded. */
  failure: Promise<string | null>;
  /** The mixer holds samples under this id, possibly from the source this entry replaced. */
  mixed: boolean;
  source: MediaSourceInput;
};

type WorkletMessage =
  | {
      generation: number;
      frames: number;
      channelCount: number;
      samples: Float32Array;
      sampleRate: number;
      startSample: number;
      type: "chunk";
    }
  | {
      generation: number;
      startSample: number;
      type: "play";
    }
  | {
      type: "pause";
    };

type WorkletEventMessage =
  | {
      message?: string;
      type: "error";
    }
  | {
      contextTime: number;
      currentSample: number;
      generation: number;
      stalled: boolean;
      type: "position";
    };

type AudioWorkerMessage =
  | {
      id: string;
      type: "remove-source";
    }
  | {
      id: string;
      samples: Float32Array;
      type: "set-source";
    }
  | {
      timeline: AudioEngineTimeline | null;
      type: "set-timeline";
    }
  | {
      chunkFrames: number;
      generation: number;
      startSample: number;
      type: "request-chunk";
    };

type AudioWorkerEventMessage = Extract<WorkletMessage, { type: "chunk" }>;

const LUMEN_AUDIO_CHUNK_FRAMES = 12_000;
const LUMEN_AUDIO_LOOKAHEAD_FRAMES = LUMEN_AUDIO_SAMPLE_RATE * 2;

export class LumenAudioEngine {
  #context: AudioContext | null = null;
  #disposed = false;
  // Every stop or restart bumps this, and async work or chunks from an older value are dropped.
  #generation = 0;
  #initialization: Promise<AudioWorkletNode | null> | null = null;
  #node: AudioWorkletNode | null = null;
  #pausedMs = 0;
  #playback: PlaybackClock | null = null;
  #queuedChunkStarts = new Set<number>();
  #sources = new Map<string, AudioSourceEntry>();
  // Only the latest sync reports failures, because a newer one may have replaced those sources.
  #sourceSync = 0;
  #timeline: AudioEngineTimeline | null = null;
  #timelineJson: string | null = null;
  #worker: Worker | null = null;
  readonly #options: LumenAudioEngineOptions;

  constructor(options: LumenAudioEngineOptions = {}) {
    this.#options = options;
  }

  /**
   * Replaces the timeline with the one the engine parsed from a newly loaded composition, and
   * rewinds, because loading a composition stops the video at frame 0.
   */
  loadAudioTimeline(timelineJson: string | null): void {
    this.#halt();
    this.#pausedMs = 0;
    if (timelineJson === this.#timelineJson) {
      return;
    }
    this.#timelineJson = timelineJson;
    this.#timeline =
      timelineJson === null ? null : (JSON.parse(timelineJson) as AudioEngineTimeline);
    this.#worker?.postMessage({ type: "set-timeline", timeline: this.#timeline });
  }

  /**
   * Rejects naming every source that failed to load. Those play silent and are retried by the
   * next sync; the others still sync. A source being loaded is shared by overlapping syncs, and
   * replacing or removing it aborts the load without waiting for it.
   */
  async syncAudioSources(registrations: Iterable<AudioSourceRegistration>): Promise<void> {
    if (this.#disposed) {
      return;
    }

    const sync = ++this.#sourceSync;
    const desired = new Map<string, AudioSourceRegistration>();
    for (const registration of registrations) {
      desired.set(registration.id, registration);
    }

    let removedMixed = false;
    for (const id of this.#sources.keys()) {
      if (!desired.has(id)) {
        removedMixed = this.#removeSource(id) || removedMixed;
      }
    }
    if (removedMixed) {
      this.#restartMix();
    }

    const loads: Promise<string | null>[] = [];
    for (const { id, source } of this.#hasClips() ? desired.values() : []) {
      const current = this.#sources.get(id);
      const entry =
        current && sameMediaInput(current.source, source)
          ? current
          : this.#startSourceLoad(id, source, current);
      loads.push(entry.failure);
    }

    const failures = (await Promise.all(loads)).filter((failure) => failure !== null);
    if (failures.length > 0 && sync === this.#sourceSync && !this.#disposed) {
      throw new Error(failures.join("\n"));
    }
  }

  #startSourceLoad(
    id: string,
    source: MediaSourceInput,
    replaced: AudioSourceEntry | undefined,
  ): AudioSourceEntry {
    replaced?.abort.abort();
    const entry: AudioSourceEntry = {
      abort: new AbortController(),
      failure: Promise.resolve(null),
      mixed: replaced?.mixed ?? false,
      source,
    };
    this.#sources.set(id, entry);
    entry.failure = this.#loadSource(id, entry);
    return entry;
  }

  async #loadSource(id: string, entry: AudioSourceEntry): Promise<string | null> {
    try {
      const samples = await decodeStereo(entry.source, entry.abort.signal);
      if (this.#sources.get(id) !== entry) {
        return null;
      }
      this.#postWorker({ type: "set-source", id, samples }, [samples.buffer as ArrayBuffer]);
      entry.mixed = true;
    } catch (error) {
      if (this.#sources.get(id) !== entry) {
        return null;
      }
      if (this.#removeSource(id)) {
        this.#restartMix();
      }
      return `audio source "${id}" could not be loaded: ${errorMessage(error)}`;
    }
    this.#restartMix();
    return null;
  }

  #restartMix(): void {
    if (this.isPlaying()) {
      this.play(this.currentTimeMs());
      this.#options.onClockChange?.();
    }
  }

  play(fromMs = this.#pausedMs): void {
    this.#halt();
    this.#pausedMs = this.#wrapMs(fromMs);
    if (this.#disposed || !this.#hasClips()) {
      return;
    }

    const generation = this.#generation;
    this.#playback = {
      contextTime: 0,
      ms: this.#pausedMs,
      stalled: true,
      startedAt: performance.now(),
      workletStarted: false,
    };
    void Promise.all([this.#ensureWorklet(), this.#getContext().resume()])
      .then(([node]) => {
        if (!node || generation !== this.#generation) {
          return;
        }
        const startSample = msToLumenAudioSample(this.currentTimeMs());
        this.#post(node, { type: "play", generation, startSample });
        this.#queueChunks(startSample);
      })
      .catch((error: unknown) => {
        if (generation === this.#generation) {
          this.#playback = null;
          console.error("[LumenAudioEngine]", error);
        }
      });
  }

  pause(): void {
    this.#pausedMs = this.currentTimeMs();
    this.#halt();
  }

  seekMs(ms: number): void {
    const wasPlaying = this.isPlaying();
    this.#halt();
    this.#pausedMs = this.#wrapMs(ms);
    if (wasPlaying) {
      this.play(this.#pausedMs);
    }
  }

  currentTimeMs(): number {
    const playback = this.#playback;
    if (!playback) {
      return this.#pausedMs;
    }
    if (!playback.workletStarted) {
      const lateMs = performance.now() - playback.startedAt - AUDIO_START_BUDGET_MS;
      return this.#wrapMs(playback.ms + Math.max(lateMs, 0));
    }
    if (playback.stalled || !this.#context) {
      return this.#wrapMs(playback.ms);
    }
    return this.#wrapMs(
      playback.ms + (this.#context.currentTime - playback.contextTime) * MS_PER_SECOND,
    );
  }

  isPlaying(): boolean {
    return this.#playback !== null;
  }

  /** Whether the position is moving. It holds through an underrun or a suspended context. */
  isClockRunning(): boolean {
    const playback = this.#playback;
    if (!playback) {
      return false;
    }
    if (!playback.workletStarted) {
      return performance.now() - playback.startedAt > AUDIO_START_BUDGET_MS;
    }
    return !playback.stalled && this.#context?.state === "running";
  }

  dispose(): void {
    this.#disposed = true;
    this.#halt();
    this.#node?.disconnect();
    this.#node = null;
    this.#initialization = null;
    this.#clearSources();
    this.#worker?.terminate();
    this.#worker = null;
    void this.#context?.close();
    this.#context = null;
  }

  #halt(): void {
    this.#generation += 1;
    this.#playback = null;
    this.#queuedChunkStarts.clear();
    if (this.#node) {
      this.#post(this.#node, { type: "pause" });
    }
  }

  #hasClips(): boolean {
    return (this.#timeline?.clips.length ?? 0) > 0;
  }

  /** Returns whether the mix changed. */
  #removeSource(id: string): boolean {
    const entry = this.#sources.get(id);
    this.#sources.delete(id);
    entry?.abort.abort();
    if (!entry?.mixed) {
      return false;
    }
    this.#postWorker({ type: "remove-source", id });
    return true;
  }

  #clearSources(): void {
    for (const entry of this.#sources.values()) {
      entry.abort.abort();
    }
    this.#sources.clear();
  }

  #ensureWorklet(): Promise<AudioWorkletNode | null> {
    this.#initialization ??= this.#createWorklet().catch((error: unknown) => {
      this.#initialization = null;
      throw error;
    });
    return this.#initialization;
  }

  async #createWorklet(): Promise<AudioWorkletNode | null> {
    const context = this.#getContext();
    const workletModuleUrl = this.#options.workletModuleUrl ?? defaultAudioWorkletUrl();
    await context.audioWorklet.addModule(String(workletModuleUrl));
    if (this.#disposed) {
      return null;
    }
    let node: AudioWorkletNode;
    try {
      node = new AudioWorkletNode(context, "lumen-audio-worklet", {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [LUMEN_AUDIO_CHANNELS],
      });
    } catch (error) {
      throw new Error(
        `failed to create Lumen audio worklet node from ${String(workletModuleUrl)}: ${String(
          error,
        )}`,
      );
    }
    node.connect(context.destination);
    node.port.onmessage = (event: MessageEvent<WorkletEventMessage>) => {
      const message = event.data;
      if (message.type === "error") {
        console.error("[LumenAudioEngine]", message.message ?? "audio worklet failed");
      } else if (message.generation === this.#generation && this.#playback) {
        const wasRunning = this.isClockRunning();
        this.#playback = {
          ...this.#playback,
          contextTime: message.contextTime,
          ms: (message.currentSample / LUMEN_AUDIO_SAMPLE_RATE) * MS_PER_SECOND,
          stalled: message.stalled,
          workletStarted: true,
        };
        this.#queueChunks(message.currentSample);
        if (this.isClockRunning() !== wasRunning) {
          this.#options.onClockChange?.();
        }
      }
    };
    this.#node = node;
    return node;
  }

  #getContext(): AudioContext {
    if (!this.#context) {
      this.#context = new AudioContext({
        latencyHint: this.#options.latencyHint ?? "interactive",
        sampleRate: LUMEN_AUDIO_SAMPLE_RATE,
      });
      this.#context.onstatechange = () => this.#options.onClockChange?.();
    }
    return this.#context;
  }

  #post(node: AudioWorkletNode, message: WorkletMessage, transfer: Transferable[] = []): void {
    node.port.postMessage(message, transfer);
  }

  #ensureAudioWorker(): Worker {
    if (this.#worker) {
      return this.#worker;
    }

    const workerModuleUrl = this.#options.workerModuleUrl ?? defaultAudioWorkerUrl();
    const worker = new Worker(String(workerModuleUrl), { type: "module" });
    worker.onmessage = (event: MessageEvent<AudioWorkerEventMessage>) => {
      const node = this.#node;
      if (!node || event.data.generation !== this.#generation) {
        return;
      }
      this.#post(node, event.data, [event.data.samples.buffer as ArrayBuffer]);
    };
    // Without a mixer no chunk ever arrives, so stop holding the video. The next sync decodes into a
    // new worker.
    worker.onerror = (event) => {
      console.error("[LumenAudioEngine]", event.message);
      this.#halt();
      worker.terminate();
      this.#worker = null;
      this.#clearSources();
      this.#options.onClockChange?.();
    };
    worker.postMessage({ type: "set-timeline", timeline: this.#timeline });
    this.#worker = worker;
    return worker;
  }

  #postWorker(message: AudioWorkerMessage, transfer: Transferable[] = []): void {
    this.#ensureAudioWorker().postMessage(message, transfer);
  }

  #wrapMs(ms: number): number {
    const timeline = this.#timeline;
    const durationMs = timeline
      ? (timeline.durationFrames / Math.max(timeline.fps, 1)) * MS_PER_SECOND
      : Number.NaN;
    return Number.isFinite(durationMs) && durationMs > 0
      ? Math.max(0, ms) % durationMs
      : Math.max(0, ms);
  }

  #queueChunks(fromSample: number): void {
    const firstChunk =
      Math.floor(Math.max(0, fromSample) / LUMEN_AUDIO_CHUNK_FRAMES) * LUMEN_AUDIO_CHUNK_FRAMES;
    for (const startSample of this.#queuedChunkStarts) {
      if (startSample < firstChunk) {
        this.#queuedChunkStarts.delete(startSample);
      }
    }

    const throughSample = fromSample + LUMEN_AUDIO_LOOKAHEAD_FRAMES;
    for (
      let startSample = firstChunk;
      startSample <= throughSample;
      startSample += LUMEN_AUDIO_CHUNK_FRAMES
    ) {
      if (this.#queuedChunkStarts.has(startSample)) {
        continue;
      }
      this.#queuedChunkStarts.add(startSample);
      this.#postWorker({
        type: "request-chunk",
        generation: this.#generation,
        startSample,
        chunkFrames: LUMEN_AUDIO_CHUNK_FRAMES,
      });
    }
  }
}

function msToLumenAudioSample(ms: number): number {
  return Math.floor((Math.max(0, ms) * LUMEN_AUDIO_SAMPLE_RATE) / MS_PER_SECOND);
}

export function defaultAudioWorkerUrl(): URL {
  return new URL("./audio-worker.js", import.meta.url);
}

export function defaultAudioWorkletUrl(): URL {
  return new URL("./audio-worklet.js", import.meta.url);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function decodeStereo(source: MediaSourceInput, signal: AbortSignal): Promise<Float32Array> {
  const blob = await sourceInputToBlob(source, signal);
  const decoder = new OfflineAudioContext(LUMEN_AUDIO_CHANNELS, 1, LUMEN_AUDIO_SAMPLE_RATE);
  const decoded = await decoder.decodeAudioData(await blob.arrayBuffer());
  const stereo =
    decoded.numberOfChannels === LUMEN_AUDIO_CHANNELS ? decoded : await renderStereo(decoded);
  return interleaveStereo(stereo);
}

/** Lets the browser apply its channel-layout-aware up and down mix. */
function renderStereo(buffer: AudioBuffer): Promise<AudioBuffer> {
  const context = new OfflineAudioContext(
    LUMEN_AUDIO_CHANNELS,
    buffer.length,
    LUMEN_AUDIO_SAMPLE_RATE,
  );
  const source = context.createBufferSource();
  source.buffer = buffer;
  source.connect(context.destination);
  source.start();
  return context.startRendering();
}

function interleaveStereo(buffer: AudioBuffer): Float32Array {
  const output = new Float32Array(buffer.length * LUMEN_AUDIO_CHANNELS);
  for (let channel = 0; channel < LUMEN_AUDIO_CHANNELS; channel += 1) {
    const samples = buffer.getChannelData(channel);
    for (let frame = 0; frame < buffer.length; frame += 1) {
      output[frame * LUMEN_AUDIO_CHANNELS + channel] = samples[frame] ?? 0;
    }
  }
  return output;
}
