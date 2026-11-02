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
  workerModuleUrl?: string | URL;
  workletModuleUrl?: string | URL;
}

/** The last position the worklet played, or the requested start until it plays anything. */
type PlaybackClock = {
  contextTime: number;
  ms: number;
  stalled: boolean;
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
  #sources = new Map<string, MediaSourceInput>();
  #sourceSync: Promise<void> = Promise.resolve();
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
   * next sync; the others still sync.
   */
  syncAudioSources(registrations: Iterable<AudioSourceRegistration>): Promise<void> {
    const desired = new Map<string, AudioSourceRegistration>();
    for (const registration of registrations) {
      desired.set(registration.id, registration);
    }

    // Serialized so overlapping syncs never decode the same source twice.
    const sync = this.#sourceSync
      .catch(() => {})
      .then(() => (this.#disposed ? undefined : this.#syncAudioSources(desired)));
    this.#sourceSync = sync;
    return sync;
  }

  async #syncAudioSources(desired: Map<string, AudioSourceRegistration>): Promise<void> {
    let changed = false;
    for (const id of this.#sources.keys()) {
      if (!desired.has(id)) {
        changed = this.#removeSource(id) || changed;
      }
    }

    const failures: string[] = [];
    for (const { id, source } of this.#hasClips() ? desired.values() : []) {
      const cached = this.#sources.get(id);
      if (cached !== undefined && sameMediaInput(cached, source)) {
        continue;
      }
      try {
        const samples = await decodeStereo(source);
        if (this.#disposed) {
          return;
        }
        this.#sources.set(id, source);
        this.#postWorker({ type: "set-source", id, samples }, [samples.buffer as ArrayBuffer]);
        changed = true;
      } catch (error) {
        if (this.#disposed) {
          return;
        }
        failures.push(`audio source "${id}" could not be loaded: ${errorMessage(error)}`);
        changed = this.#removeSource(id) || changed;
      }
    }

    if (changed && this.isPlaying()) {
      this.play(this.currentTimeMs());
    }
    if (failures.length > 0) {
      throw new Error(failures.join("\n"));
    }
  }

  play(fromMs = this.#pausedMs): void {
    this.#halt();
    this.#pausedMs = this.#wrapMs(fromMs);
    if (this.#disposed || !this.#hasClips()) {
      return;
    }

    const generation = this.#generation;
    const startSample = msToLumenAudioSample(this.#pausedMs);
    this.#playback = { contextTime: 0, ms: this.#pausedMs, stalled: true };
    void this.#getContext().resume();
    void this.#ensureWorklet()
      .then((node) => {
        if (!node || generation !== this.#generation) {
          return;
        }
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

  dispose(): void {
    this.#disposed = true;
    this.#halt();
    this.#node?.disconnect();
    this.#node = null;
    this.#initialization = null;
    this.#sources.clear();
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

  #removeSource(id: string): boolean {
    if (!this.#sources.delete(id)) {
      return false;
    }
    this.#postWorker({ type: "remove-source", id });
    return true;
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
        this.#playback = {
          contextTime: message.contextTime,
          ms: (message.currentSample / LUMEN_AUDIO_SAMPLE_RATE) * MS_PER_SECOND,
          stalled: message.stalled,
        };
        this.#queueChunks(message.currentSample);
      }
    };
    this.#node = node;
    return node;
  }

  #getContext(): AudioContext {
    this.#context ??= new AudioContext({
      latencyHint: this.#options.latencyHint ?? "interactive",
      sampleRate: LUMEN_AUDIO_SAMPLE_RATE,
    });
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
    // Without a mixer no chunk ever arrives, so stop holding the video on an audio clock. The next
    // sync decodes into a new worker.
    worker.onerror = (event) => {
      console.error("[LumenAudioEngine]", event.message);
      this.#halt();
      worker.terminate();
      this.#worker = null;
      this.#sources.clear();
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

async function decodeStereo(source: MediaSourceInput): Promise<Float32Array> {
  const blob = await sourceInputToBlob(source);
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
