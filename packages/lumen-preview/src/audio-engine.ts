import type { MediaSourceInput } from "./media/index.js";
import { sourceInputToBlob } from "./media/index.js";

export const LUMEN_AUDIO_SAMPLE_RATE = 48_000;
export const LUMEN_AUDIO_CHANNELS = 2;
export const LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES = 384;
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

type CompositionAudioTrack = {
  id?: unknown;
  muted?: unknown;
  solo?: unknown;
  volume?: unknown;
};

type CompositionAudioClip = {
  duration_frames?: unknown;
  duration_ms?: unknown;
  durationFrames?: unknown;
  durationMs?: unknown;
  id?: unknown;
  source_id?: unknown;
  source_start_ms?: unknown;
  source_start_seconds?: unknown;
  sourceId?: unknown;
  sourceStartMs?: unknown;
  start_frame?: unknown;
  start_ms?: unknown;
  startFrame?: unknown;
  startMs?: unknown;
  track_id?: unknown;
  trackId?: unknown;
  volume?: unknown;
};

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

type ActivePlayback = {
  startContextTime: number;
  startMs: number;
};

export interface ScheduledAudioClip {
  clip: AudioEngineClip;
  delaySeconds: number;
  durationSeconds: number;
  fadeSeconds: number;
  gain: number;
  sourceOffsetSeconds: number;
}

type WorkletMessage =
  | {
      generation: number;
      startSample?: number;
      type: "reset";
    }
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
    }
  | {
      generation: number;
      startSample: number;
      type: "seek";
    };

type WorkletEventMessage =
  | {
      message?: string;
      type: "error" | "ready";
    }
  | {
      currentSample: number;
      generation: number;
      type: "need-chunks";
    };

type AudioWorkerMessage =
  | {
      type: "clear-sources";
    }
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
      throughSample: number;
      type: "request-chunks";
    };

type AudioWorkerEventMessage = Extract<WorkletMessage, { type: "chunk" }>;

const LUMEN_AUDIO_CHUNK_FRAMES = 12_000;
const LUMEN_AUDIO_LOOKAHEAD_FRAMES = LUMEN_AUDIO_SAMPLE_RATE * 2;

export class LumenAudioEngine {
  #context: AudioContext | null = null;
  #initialization: Promise<AudioWorkletNode> | null = null;
  #node: AudioWorkletNode | null = null;
  #worker: Worker | null | undefined;
  #pausedMs = 0;
  #playback: ActivePlayback | null = null;
  #timeline: AudioEngineTimeline | null = null;
  #buffering = false;
  #generation = 0;
  #queuedChunkStarts = new Set<number>();
  #workerSourceIds = new Set<string>();
  #sources = new Map<string, Float32Array>();
  readonly #options: LumenAudioEngineOptions;

  constructor(options: LumenAudioEngineOptions = {}) {
    this.#options = options;
  }

  async syncAudioSources(registrations: Iterable<AudioSourceRegistration>): Promise<void> {
    const desired = new Map<string, AudioSourceRegistration>();
    for (const registration of registrations) {
      desired.set(registration.id, registration);
    }

    const worker = this.#ensureAudioWorker();
    if (desired.size === 0 && !this.#node && !worker) {
      this.#sources.clear();
      this.#workerSourceIds.clear();
      return;
    }

    await this.#ensureWorklet();
    let changed = false;
    for (const id of worker ? this.#workerSourceIds : this.#sources.keys()) {
      if (!desired.has(id)) {
        this.#sources.delete(id);
        this.#workerSourceIds.delete(id);
        this.#postWorker({ type: "remove-source", id });
        changed = true;
      }
    }

    for (const registration of desired.values()) {
      if (
        worker ? this.#workerSourceIds.has(registration.id) : this.#sources.has(registration.id)
      ) {
        continue;
      }
      const samples = await this.#decodeSource(registration.source);
      if (worker) {
        this.#workerSourceIds.add(registration.id);
        this.#postWorker(
          {
            type: "set-source",
            id: registration.id,
            samples,
          },
          [samples.buffer as ArrayBuffer],
        );
      } else {
        this.#sources.set(registration.id, samples);
      }
      changed = true;
    }

    if (changed) {
      const currentSample = this.#currentSample();
      this.#resetAudioQueue(currentSample);
      this.#queueChunks(currentSample);
    }
  }

  setAudioTimeline(timeline: AudioEngineTimeline | null): void {
    this.#timeline = timeline;
    this.#postWorker({ type: "set-timeline", timeline });
    if (this.#node) {
      const currentSample = this.#currentSample();
      this.#resetAudioQueue(currentSample);
      this.#queueChunks(currentSample);
    }
  }

  async preloadAudioWindow(timeMs: number, durationMs = 2_000): Promise<void> {
    if (!this.#timeline || this.#timeline.clips.length === 0) {
      this.#buffering = false;
      return;
    }
    await this.#ensureWorklet();
    this.#queueChunks(msToLumenAudioSample(timeMs), msToLumenAudioSample(timeMs + durationMs));
    this.#buffering = false;
  }

  play(fromMs = this.#pausedMs): void {
    this.stop();
    if (!this.#timeline || this.#timeline.clips.length === 0) {
      this.#pausedMs = Math.max(0, fromMs);
      return;
    }
    this.#pausedMs = Math.max(0, fromMs);
    const startSample = msToLumenAudioSample(this.#pausedMs);
    void this.#ensureWorklet()
      .then((node) => {
        const context = this.#getContext();
        void context.resume();
        this.#resetAudioQueue(startSample);
        this.#queueChunks(startSample);
        this.#playback = {
          startContextTime: context.currentTime,
          startMs: this.#pausedMs,
        };
        this.#post(node, { type: "play", generation: this.#generation, startSample });
      })
      .catch((error: unknown) => this.#reportAsyncError(error));
  }

  pause(): void {
    this.#pausedMs = this.currentTimeMs();
    this.#playback = null;
    if (this.#node) {
      this.#post(this.#node, { type: "pause" });
    }
  }

  stop(): void {
    this.#playback = null;
    if (this.#node) {
      this.#post(this.#node, { type: "pause" });
    }
  }

  seekMs(ms: number): void {
    const wasPlaying = this.#playback !== null;
    this.#pausedMs = Math.max(0, ms);
    const startSample = msToLumenAudioSample(this.#pausedMs);
    this.#playback = null;
    if (!this.#node) {
      return;
    }
    void this.#ensureWorklet()
      .then((node) => {
        this.#generation += 1;
        this.#queuedChunkStarts.clear();
        this.#post(node, { type: "seek", generation: this.#generation, startSample });
        this.#queueChunks(startSample);
        if (wasPlaying) {
          this.play(this.#pausedMs);
        }
      })
      .catch((error: unknown) => this.#reportAsyncError(error));
  }

  currentTimeMs(): number {
    const playback = this.#playback;
    const context = this.#context;
    if (!playback || !context) {
      return this.#pausedMs;
    }

    const durationMs = this.#timeline
      ? (this.#timeline.durationFrames / Math.max(this.#timeline.fps, 1)) * MS_PER_SECOND
      : Number.POSITIVE_INFINITY;
    return Math.min(
      durationMs,
      playback.startMs + (context.currentTime - playback.startContextTime) * MS_PER_SECOND,
    );
  }

  isBuffering(): boolean {
    return this.#buffering;
  }

  isPlaying(): boolean {
    return this.#playback !== null;
  }

  dispose(): void {
    this.#playback = null;
    const node = this.#node;
    if (node) {
      this.#post(node, { type: "reset", generation: this.#generation + 1 });
      this.#post(node, { type: "pause" });
      node.disconnect();
    }
    this.#node = null;
    this.#sources.clear();
    this.#workerSourceIds.clear();
    this.#worker?.terminate();
    this.#worker = null;
    void this.#context?.close();
    this.#context = null;
    this.#initialization = null;
  }

  async #decodeSource(source: MediaSourceInput): Promise<Float32Array> {
    const blob = await sourceInputToBlob(source);
    const bytes = await blob.arrayBuffer();
    const decoded = await this.#getContext().decodeAudioData(bytes);
    const resampled =
      decoded.sampleRate === LUMEN_AUDIO_SAMPLE_RATE
        ? decoded
        : await resampleAudioBuffer(decoded, LUMEN_AUDIO_SAMPLE_RATE);
    return interleaveAudioBuffer(resampled, LUMEN_AUDIO_CHANNELS);
  }

  async #ensureWorklet(): Promise<AudioWorkletNode> {
    if (this.#node) {
      return this.#node;
    }
    this.#initialization ??= this.#createWorklet();
    return this.#initialization;
  }

  async #createWorklet(): Promise<AudioWorkletNode> {
    const context = this.#getContext();
    const workletModuleUrl = this.#options.workletModuleUrl ?? defaultAudioWorkletUrl();
    this.#ensureAudioWorker();
    await context.audioWorklet.addModule(String(workletModuleUrl));
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
      if (event.data.type === "error") {
        console.error("[LumenAudioEngine]", event.data.message ?? "audio worklet failed");
      }
      if (event.data.type === "ready") {
        console.debug("[LumenAudioEngine] audio worklet ready");
      }
      if (event.data.type === "need-chunks" && event.data.generation === this.#generation) {
        this.#queueChunks(event.data.currentSample);
      }
    };
    this.#node = node;
    this.#resetAudioQueue(this.#currentSample());
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

  #ensureAudioWorker(): Worker | null {
    if (this.#worker !== undefined) {
      return this.#worker;
    }

    const workerModuleUrl = this.#options.workerModuleUrl ?? defaultAudioWorkerUrl();
    if (!workerModuleUrl || typeof Worker === "undefined") {
      this.#worker = null;
      return null;
    }

    const worker = new Worker(String(workerModuleUrl), { type: "module" });
    worker.onmessage = (event: MessageEvent<AudioWorkerEventMessage>) => {
      const node = this.#node;
      if (!node || event.data.generation !== this.#generation) {
        return;
      }
      this.#post(node, event.data, [event.data.samples.buffer as ArrayBuffer]);
    };
    worker.onerror = (event) => {
      console.error("[LumenAudioEngine]", event.message);
    };
    this.#worker = worker;
    this.#postWorker({ type: "set-timeline", timeline: this.#timeline });
    return worker;
  }

  #postWorker(message: AudioWorkerMessage, transfer: Transferable[] = []): void {
    this.#worker?.postMessage(message, transfer);
  }

  #currentSample(): number {
    return msToLumenAudioSample(this.currentTimeMs());
  }

  #resetAudioQueue(startSample: number): void {
    this.#generation += 1;
    this.#queuedChunkStarts.clear();
    if (this.#node) {
      this.#post(this.#node, {
        type: "reset",
        generation: this.#generation,
        startSample,
      });
    }
  }

  #queueChunks(
    fromSample: number,
    throughSample = fromSample + LUMEN_AUDIO_LOOKAHEAD_FRAMES,
  ): void {
    const node = this.#node;
    const timeline = this.#timeline;
    if (!node || !timeline || timeline.clips.length === 0) {
      return;
    }

    const chunkStartSample =
      Math.floor(Math.max(0, fromSample) / LUMEN_AUDIO_CHUNK_FRAMES) * LUMEN_AUDIO_CHUNK_FRAMES;
    for (
      let startSample = chunkStartSample;
      startSample <= throughSample;
      startSample += LUMEN_AUDIO_CHUNK_FRAMES
    ) {
      if (this.#queuedChunkStarts.has(startSample)) {
        continue;
      }

      this.#queuedChunkStarts.add(startSample);
      if (this.#worker) {
        this.#postWorker({
          type: "request-chunks",
          generation: this.#generation,
          startSample,
          throughSample: startSample,
          chunkFrames: LUMEN_AUDIO_CHUNK_FRAMES,
        });
      } else {
        const samples = mixTimelineChunk(
          timeline,
          this.#sources,
          startSample,
          LUMEN_AUDIO_CHUNK_FRAMES,
        );
        this.#post(
          node,
          {
            type: "chunk",
            generation: this.#generation,
            startSample,
            frames: LUMEN_AUDIO_CHUNK_FRAMES,
            sampleRate: LUMEN_AUDIO_SAMPLE_RATE,
            channelCount: LUMEN_AUDIO_CHANNELS,
            samples,
          },
          [samples.buffer as ArrayBuffer],
        );
      }
    }
  }

  #reportAsyncError(error: unknown): void {
    console.error("[LumenAudioEngine]", error);
  }
}

export function createLumenAudioSchedule(
  timeline: AudioEngineTimeline,
  fromMs: number,
): ScheduledAudioClip[] {
  const startSample = msToLumenAudioSample(Math.max(0, fromMs));
  const hasSolo = timeline.tracks.some((track) => track.solo);
  const tracks = new Map(timeline.tracks.map((track) => [track.id, track]));
  const scheduled: ScheduledAudioClip[] = [];

  for (const clip of timeline.clips) {
    const track = tracks.get(clip.trackId);
    if (
      !track ||
      track.muted ||
      (hasSolo && !track.solo) ||
      track.volume <= 0 ||
      clip.volume <= 0
    ) {
      continue;
    }

    const clipStartSample = msToLumenAudioSample(clip.startMs);
    const clipDurationSamples = Math.max(1, msToLumenAudioSample(clip.durationMs));
    const clipEndSample = clipStartSample + clipDurationSamples;
    if (clipEndSample <= startSample) {
      continue;
    }

    const overlapStartSample = Math.max(startSample, clipStartSample);
    const clipOffsetSamples = overlapStartSample - clipStartSample;
    const remainingSamples = clipDurationSamples - clipOffsetSamples;
    const delaySamples = Math.max(0, clipStartSample - startSample);
    const sourceStartSample = msToLumenAudioSample(clip.sourceStartMs) + clipOffsetSamples;

    scheduled.push({
      clip,
      delaySeconds: lumenAudioSamplesToSeconds(delaySamples),
      durationSeconds: lumenAudioSamplesToSeconds(remainingSamples),
      fadeSeconds: lumenAudioSamplesToSeconds(
        Math.min(LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES, Math.max(0, Math.floor(remainingSamples / 4))),
      ),
      gain: track.volume * clip.volume,
      sourceOffsetSeconds: lumenAudioSamplesToSeconds(sourceStartSample),
    });
  }

  return scheduled;
}

export function audioTimelineFromCompositionJson(
  compositionJson: string | null | undefined,
): AudioEngineTimeline | null {
  if (!compositionJson) {
    return null;
  }

  let composition: unknown;
  try {
    composition = JSON.parse(compositionJson);
  } catch {
    return null;
  }

  if (!isRecord(composition)) {
    return null;
  }

  const audio = composition.audio;
  const timeline = composition.timeline;
  if (!isRecord(audio) || !isRecord(timeline)) {
    return null;
  }

  const tracksInput = Array.isArray(audio.tracks) ? audio.tracks : [];
  const clipsInput = Array.isArray(audio.clips) ? audio.clips : [];
  const fps = numberOr(timeline.fps, 30);
  const durationFrames = integerOr(timeline.duration_frames, 1);

  const tracks = tracksInput.filter(isRecord).map((track): AudioEngineTrack => {
    const input = track as CompositionAudioTrack;
    return {
      id: stringOr(input.id, "track"),
      muted: booleanOr(input.muted, false),
      solo: booleanOr(input.solo, false),
      volume: numberOr(input.volume, 1),
    };
  });

  const clips = clipsInput
    .filter(isRecord)
    .map((clip): AudioEngineClip | null => {
      const input = clip as CompositionAudioClip;
      const durationMs = audioDurationMs(input, fps);
      if (durationMs <= 0) {
        return null;
      }

      return {
        durationMs,
        id: stringOr(input.id, "clip"),
        sourceId: stringOr(input.source_id ?? input.sourceId, ""),
        sourceStartMs: audioSourceStartMs(input),
        startMs: audioStartMs(input, fps),
        trackId: stringOr(input.track_id ?? input.trackId, ""),
        volume: numberOr(input.volume, 1),
      };
    })
    .filter((clip): clip is AudioEngineClip => Boolean(clip?.sourceId && clip.trackId));

  if (tracks.length === 0 && clips.length === 0) {
    return null;
  }

  return {
    clips,
    durationFrames,
    fps,
    tracks,
  };
}

export function msToLumenAudioSample(ms: number): number {
  return Math.floor((Math.max(0, ms) * LUMEN_AUDIO_SAMPLE_RATE) / MS_PER_SECOND);
}

export function lumenAudioSamplesToSeconds(samples: number): number {
  return Math.max(0, samples) / LUMEN_AUDIO_SAMPLE_RATE;
}

export function defaultAudioWorkerUrl(): URL {
  return new URL("./audio-worker.js", import.meta.url);
}

export function defaultAudioWorkletUrl(): URL {
  return new URL("./audio-worklet.js", import.meta.url);
}

export function mixTimelineChunk(
  timeline: AudioEngineTimeline,
  sources: ReadonlyMap<string, Float32Array>,
  startSample: number,
  frames: number,
): Float32Array {
  const output = new Float32Array(frames * LUMEN_AUDIO_CHANNELS);
  const hasSolo = timeline.tracks.some((track) => track.solo);
  const tracks = new Map(timeline.tracks.map((track) => [track.id, track]));
  const endSample = startSample + frames;

  for (const clip of timeline.clips) {
    const track = tracks.get(clip.trackId);
    if (
      !track ||
      track.muted ||
      (hasSolo && !track.solo) ||
      track.volume <= 0 ||
      clip.volume <= 0
    ) {
      continue;
    }

    const source = sources.get(clip.sourceId);
    if (!source) {
      continue;
    }

    const clipStartSample = msToLumenAudioSample(clip.startMs);
    const clipDurationSamples = Math.max(1, msToLumenAudioSample(clip.durationMs));
    const clipEndSample = clipStartSample + clipDurationSamples;
    const overlapStartSample = Math.max(startSample, clipStartSample);
    const overlapEndSample = Math.min(endSample, clipEndSample);
    if (overlapEndSample <= overlapStartSample) {
      continue;
    }

    const baseGain = track.volume * clip.volume;
    const fadeSamples = Math.min(
      LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES,
      Math.max(0, Math.floor(clipDurationSamples / 4)),
    );
    const sourceStartSample = msToLumenAudioSample(clip.sourceStartMs);
    for (let sample = overlapStartSample; sample < overlapEndSample; sample += 1) {
      const clipOffset = sample - clipStartSample;
      const sourceFrame = sourceStartSample + clipOffset;
      if (sourceFrame < 0 || sourceFrame * LUMEN_AUDIO_CHANNELS >= source.length) {
        continue;
      }

      const gain = baseGain * edgeFadeGain(clipOffset, clipDurationSamples, fadeSamples);
      const outputFrame = sample - startSample;
      for (let channel = 0; channel < LUMEN_AUDIO_CHANNELS; channel += 1) {
        const outputIndex = outputFrame * LUMEN_AUDIO_CHANNELS + channel;
        const sourceIndex = sourceFrame * LUMEN_AUDIO_CHANNELS + channel;
        output[outputIndex] = clampAudioSample(
          (output[outputIndex] ?? 0) + (source[sourceIndex] ?? 0) * gain,
        );
      }
    }
  }

  return output;
}

function interleaveAudioBuffer(buffer: AudioBuffer, channelCount: number): Float32Array {
  const frames = buffer.length;
  const output = new Float32Array(frames * channelCount);
  for (let frame = 0; frame < frames; frame += 1) {
    for (let channel = 0; channel < channelCount; channel += 1) {
      const source = buffer.getChannelData(Math.min(channel, buffer.numberOfChannels - 1));
      output[frame * channelCount + channel] = source[frame] ?? 0;
    }
  }
  return output;
}

function audioStartMs(input: CompositionAudioClip, fps: number): number {
  if (typeof input.start_ms === "number") {
    return Math.max(0, input.start_ms);
  }
  if (typeof input.startMs === "number") {
    return Math.max(0, input.startMs);
  }
  return framesToMs(input.start_frame ?? input.startFrame, fps);
}

function audioDurationMs(input: CompositionAudioClip, fps: number): number {
  if (typeof input.duration_ms === "number") {
    return Math.max(0, input.duration_ms);
  }
  if (typeof input.durationMs === "number") {
    return Math.max(0, input.durationMs);
  }
  return framesToMs(input.duration_frames ?? input.durationFrames, fps);
}

function audioSourceStartMs(input: CompositionAudioClip): number {
  if (typeof input.source_start_ms === "number") {
    return Math.max(0, input.source_start_ms);
  }
  if (typeof input.sourceStartMs === "number") {
    return Math.max(0, input.sourceStartMs);
  }
  if (typeof input.source_start_seconds === "number") {
    return Math.max(0, input.source_start_seconds * MS_PER_SECOND);
  }
  return 0;
}

function framesToMs(value: unknown, fps: number): number {
  if (typeof value !== "number" || fps <= 0) {
    return 0;
  }
  return Math.max(0, (value / fps) * MS_PER_SECOND);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function booleanOr(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function integerOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.max(1, Math.floor(value))
    : fallback;
}

function numberOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function stringOr(value: unknown, fallback: string): string {
  return typeof value === "string" ? value : fallback;
}

async function resampleAudioBuffer(buffer: AudioBuffer, sampleRate: number): Promise<AudioBuffer> {
  const length = Math.ceil((buffer.length * sampleRate) / buffer.sampleRate);
  const context = new OfflineAudioContext(buffer.numberOfChannels, length, sampleRate);
  const source = context.createBufferSource();
  source.buffer = buffer;
  source.connect(context.destination);
  source.start();
  return context.startRendering();
}

function edgeFadeGain(offset: number, duration: number, fadeSamples: number): number {
  if (fadeSamples <= 0) {
    return 1;
  }
  const fadeIn = Math.min(1, offset / fadeSamples);
  const fadeOut = Math.min(1, (duration - offset) / fadeSamples);
  return Math.max(0, Math.min(fadeIn, fadeOut));
}

function clampAudioSample(sample: number): number {
  if (sample > 1) {
    return 1;
  }
  if (sample < -1) {
    return -1;
  }
  return sample;
}
