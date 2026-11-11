import { sameMediaInput, sourceInputToBlob } from "./media/source.js";
import type { MediaSourceInput } from "./media/types.js";
import { assetHint } from "./asset-hint.js";
import { createModuleWorker } from "./module-worker.js";
import { mod } from "./playback-range.js";

const AUDIO_FALLBACK =
  "If it isn't served there, serve `dist/audio-worker.js` from `@lumiscia/lumen-preview` beside `dist/audio-engine.js`, or pass `audio.workerModuleUrl` to `mountPreview`.";

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

/** The part of the timeline that plays, `endMs` exclusive. It repeats when `loop` is set. */
export interface AudioPlaybackRange {
  endMs: number;
  loop: boolean;
  startMs: number;
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
  /** Called once when audio stops arriving mid-playback. The position keeps moving without it. */
  onError?: (message: string) => void;
  workerModuleUrl?: string | URL;
  workletModuleUrl?: string | URL;
}

// How long a playback holds for audio that hasn't started, such as on a page with no user gesture
// yet or with a mixer that has nothing to send. After that the position runs without it for the rest
// of the playback, and audio joins at that position if it ever arrives.
const AUDIO_START_BUDGET_MS = 1_000;

// How quickly the output level follows a volume or mute change. About 50 ms to settle, which is too
// slow to click and too fast to notice.
const GAIN_TIME_CONSTANT_S = 0.015;

// How long the worklet may report an underrun before the audio is given up on. A mix restarted by a
// source that is still arriving over a slow connection can take several seconds to refill, so this
// is well past that while still surfacing a dead mixer within one breath of the user noticing.
export const AUDIO_STALL_TIMEOUT_MS = 10_000;

/** The last position the worklet reported, or the requested start until it has reported one. */
type PlaybackClock = {
  contextTime: number;
  /** When the budget for holding ends. */
  holdUntil: number;
  ms: number;
  stalled: boolean;
  workletReported: boolean;
};

function msPastHold(playback: PlaybackClock): number {
  return Math.max(performance.now() - playback.holdUntil, 0);
}

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
      holdFrames: number;
      rate: number;
      rateRevision: number;
      startSample: number;
      type: "play";
    }
  | {
      type: "pause";
    }
  | {
      rate: number;
      rateRevision: number;
      type: "rate";
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
      rate: number;
      /** The rate change the worklet had heard when it reported. */
      rateRevision: number;
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
      range: AudioPlaybackRange | null;
      type: "set-range";
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
  #gain: GainNode | null = null;
  // Every stop or restart bumps this, and async work or chunks from an older value are dropped.
  #generation = 0;
  #initialization: Promise<AudioWorkletNode | null> | null = null;
  #muted = false;
  #node: AudioWorkletNode | null = null;
  #pausedMs = 0;
  #playback: PlaybackClock | null = null;
  #queuedChunkStarts = new Set<number>();
  #range: AudioPlaybackRange | null = null;
  #rate = 1;
  // Bumped by every rate change, so a report from before one is told apart even at the same rate.
  #rateRevision = 0;
  #sources = new Map<string, AudioSourceEntry>();
  // Only the latest sync reports failures, because a newer one may have replaced those sources.
  #sourceSync = 0;
  #audioAbandoned = false;
  #stallTimer: ReturnType<typeof setTimeout> | null = null;
  #timeline: AudioEngineTimeline | null = null;
  #timelineJson: string | null = null;
  #volume = 1;
  #worker: Worker | null = null;
  readonly #options: LumenAudioEngineOptions;

  constructor(options: LumenAudioEngineOptions = {}) {
    this.#options = options;
  }

  /**
   * Replaces the timeline with the one the engine parsed from a newly loaded composition. An
   * identical timeline leaves playback running. A different one stops it and parks at 0, so the
   * caller plays or seeks to where the video is. Returns whether the timeline changed.
   */
  loadAudioTimeline(timelineJson: string | null): boolean {
    if (timelineJson === this.#timelineJson) {
      return false;
    }
    this.#halt();
    this.#pausedMs = 0;
    this.#timelineJson = timelineJson;
    this.#timeline =
      timelineJson === null ? null : (JSON.parse(timelineJson) as AudioEngineTimeline);
    this.#worker?.postMessage({ type: "set-timeline", timeline: this.#timeline });
    return true;
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

  #restartMix(positionMs = this.currentTimeMs()): void {
    if (this.#playback && !this.#audioAbandoned) {
      const holdUntil = Math.max(this.#playback.holdUntil, performance.now());
      this.#start(positionMs, holdUntil);
      this.#options.onClockChange?.();
    }
  }

  /**
   * Plays the timeline `rate` times as fast, with pitch following speed. The position carries on
   * from where it is, without a restart.
   */
  setRate(rate: number): void {
    if (rate === this.#rate) {
      return;
    }
    const playback = this.#playback;
    if (playback) {
      this.#playback = this.#rebased(playback);
    }
    this.#rate = rate;
    this.#rateRevision += 1;
    if (playback && this.#node) {
      this.#post(this.#node, { type: "rate", rate, rateRevision: this.#rateRevision });
    }
  }

  /**
   * Plays only this part of the timeline, wrapping round it or stopping at its end. Null plays the
   * whole timeline, looping. Audio playing restarts at its position, which the range fits.
   */
  setRange(range: AudioPlaybackRange | null): void {
    if (
      range?.startMs === this.#range?.startMs &&
      range?.endMs === this.#range?.endMs &&
      range?.loop === this.#range?.loop
    ) {
      return;
    }
    const positionMs = this.currentTimeMs();
    this.#range = range;
    this.#worker?.postMessage({ type: "set-range", range });
    this.#pausedMs = this.#fitMs(this.#pausedMs);
    if (this.#audioAbandoned && this.#playback) {
      // The elapsed time counted under the old range must not be read again under the new one.
      this.#playback = { ...this.#playback, holdUntil: performance.now(), ms: positionMs };
    }
    this.#restartMix(positionMs);
  }

  /** The output level is `volume`, or silence while `muted`. Either changes without a click. */
  setVolume(volume: number, muted: boolean): void {
    this.#volume = volume;
    this.#muted = muted;
    if (this.#gain && this.#context) {
      this.#gain.gain.setTargetAtTime(
        this.#gainTarget(),
        this.#context.currentTime,
        GAIN_TIME_CONSTANT_S,
      );
    }
  }

  #gainTarget(): number {
    return this.#muted ? 0 : this.#volume;
  }

  play(fromMs = this.#pausedMs): void {
    this.#start(fromMs, performance.now() + AUDIO_START_BUDGET_MS);
  }

  #start(fromMs: number, holdUntil: number): void {
    this.#halt();
    this.#pausedMs = this.#fitMs(fromMs);
    if (this.#disposed || !this.#hasClips()) {
      return;
    }

    const generation = this.#generation;
    this.#playback = {
      contextTime: 0,
      holdUntil,
      ms: this.#pausedMs,
      stalled: true,
      workletReported: false,
    };
    void Promise.all([this.#ensureWorklet(), this.#getContext().resume()])
      .then(([node]) => {
        if (!node || generation !== this.#generation) {
          return;
        }
        const startSample = msToLumenAudioSample(this.currentTimeMs());
        const holdFrames = msToLumenAudioSample(Math.max(holdUntil - performance.now(), 0));
        this.#post(node, {
          type: "play",
          generation,
          holdFrames,
          rate: this.#rate,
          rateRevision: this.#rateRevision,
          startSample,
        });
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
    this.#pausedMs = this.#fitMs(ms);
    if (wasPlaying) {
      this.play(this.#pausedMs);
    }
  }

  currentTimeMs(): number {
    const playback = this.#playback;
    return playback ? this.#fitMs(this.#movedMs(playback)) : this.#pausedMs;
  }

  /** Where the playback has got to, counting on past the end of the range it repeats. */
  #movedMs(playback: PlaybackClock): number {
    if (!playback.workletReported) {
      return playback.ms + msPastHold(playback) * this.#rate;
    }
    if (playback.stalled || !this.#context) {
      return playback.ms;
    }
    return (
      playback.ms + (this.#context.currentTime - playback.contextTime) * MS_PER_SECOND * this.#rate
    );
  }

  /** The same position, counted from now, so a different rate applies only from here on. */
  #rebased(playback: PlaybackClock): PlaybackClock {
    const ms = this.#movedMs(playback);
    return playback.workletReported && this.#context
      ? { ...playback, contextTime: this.#context.currentTime, ms }
      : { ...playback, holdUntil: Math.max(playback.holdUntil, performance.now()), ms };
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
    if (!playback.workletReported) {
      return performance.now() >= playback.holdUntil;
    }
    return !playback.stalled && this.#context?.state === "running";
  }

  dispose(): void {
    this.#disposed = true;
    this.#halt();
    this.#node?.disconnect();
    this.#node = null;
    this.#gain?.disconnect();
    this.#gain = null;
    this.#initialization = null;
    this.#clearSources();
    this.#worker?.terminate();
    this.#worker = null;
    void this.#context?.close();
    this.#context = null;
  }

  #halt(): void {
    this.#audioAbandoned = false;
    this.#clearStallTimer();
    this.#generation += 1;
    this.#playback = null;
    this.#queuedChunkStarts.clear();
    if (this.#node) {
      this.#post(this.#node, { type: "pause" });
    }
  }

  #clearStallTimer(): void {
    if (this.#stallTimer !== null) {
      clearTimeout(this.#stallTimer);
      this.#stallTimer = null;
    }
  }

  // Only a reported underrun on a running context counts, so suspension restarts the timeout.
  #syncStallTimer(): void {
    const playback = this.#playback;
    if (!playback?.workletReported || !playback.stalled || this.#context?.state !== "running") {
      this.#clearStallTimer();
      return;
    }
    this.#stallTimer ??= setTimeout(() => {
      this.#stallTimer = null;
      this.#abandonAudio();
    }, AUDIO_STALL_TIMEOUT_MS);
  }

  // Later worklet reports and chunks belong to the old generation, so audio stays off until the
  // next play, seek or composition load, and the position runs on the clock used before audio
  // starts.
  #abandonAudio(): void {
    const playback = this.#playback;
    if (!playback) {
      return;
    }
    this.#audioAbandoned = true;
    this.#generation += 1;
    this.#queuedChunkStarts.clear();
    if (this.#node) {
      this.#post(this.#node, { type: "pause" });
    }
    this.#playback = { ...playback, holdUntil: performance.now(), workletReported: false };
    this.#options.onError?.(
      "audio stopped arriving during playback; the video continues without it",
    );
    this.#options.onClockChange?.();
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
    node.connect(this.#outputGain());
    node.port.onmessage = (event: MessageEvent<WorkletEventMessage>) => {
      const message = event.data;
      if (message.type === "error") {
        console.error("[LumenAudioEngine]", message.message ?? "audio worklet failed");
      } else if (
        message.generation === this.#generation &&
        message.rateRevision === this.#rateRevision &&
        this.#playback
      ) {
        const wasRunning = this.isClockRunning();
        this.#playback = {
          ...this.#playback,
          contextTime: message.contextTime,
          ms: (message.currentSample / LUMEN_AUDIO_SAMPLE_RATE) * MS_PER_SECOND,
          stalled: message.stalled,
          workletReported: true,
        };
        this.#queueChunks(message.currentSample);
        this.#syncStallTimer();
        if (this.isClockRunning() !== wasRunning) {
          this.#options.onClockChange?.();
        }
      }
    };
    this.#node = node;
    return node;
  }

  #outputGain(): GainNode {
    if (!this.#gain) {
      const context = this.#getContext();
      this.#gain = context.createGain();
      this.#gain.gain.value = this.#gainTarget();
      this.#gain.connect(context.destination);
    }
    return this.#gain;
  }

  #getContext(): AudioContext {
    if (!this.#context) {
      this.#context = new AudioContext({
        latencyHint: this.#options.latencyHint ?? "interactive",
        sampleRate: LUMEN_AUDIO_SAMPLE_RATE,
      });
      this.#context.onstatechange = () => {
        this.#syncStallTimer();
        this.#options.onClockChange?.();
      };
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
    const worker = createModuleWorker(workerModuleUrl);
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
      const url = String(workerModuleUrl);
      console.error(
        "[LumenAudioEngine]",
        `the audio worker failed at ${url}: ${event.message}. ${assetHint(url, AUDIO_FALLBACK)}`,
      );
      this.#halt();
      worker.terminate();
      this.#worker = null;
      this.#clearSources();
      this.#options.onClockChange?.();
    };
    worker.postMessage({ type: "set-timeline", timeline: this.#timeline });
    worker.postMessage({ type: "set-range", range: this.#range });
    this.#worker = worker;
    return worker;
  }

  #postWorker(message: AudioWorkerMessage, transfer: Transferable[] = []): void {
    this.#ensureAudioWorker().postMessage(message, transfer);
  }

  /** Wrapped round a looping range, or held inside one that isn't. */
  #fitMs(ms: number): number {
    const range = this.#range ?? this.#wholeTimeline();
    if (!range || !(range.endMs > range.startMs)) {
      return Math.max(0, ms);
    }
    return range.loop
      ? range.startMs + mod(ms - range.startMs, range.endMs - range.startMs)
      : Math.min(Math.max(ms, range.startMs), range.endMs);
  }

  #wholeTimeline(): AudioPlaybackRange | null {
    const timeline = this.#timeline;
    return timeline
      ? {
          endMs: (timeline.durationFrames / Math.max(timeline.fps, 1)) * MS_PER_SECOND,
          loop: true,
          startMs: 0,
        }
      : null;
  }

  #queueChunks(fromSample: number): void {
    const firstChunk =
      Math.floor(Math.max(0, fromSample) / LUMEN_AUDIO_CHUNK_FRAMES) * LUMEN_AUDIO_CHUNK_FRAMES;
    for (const startSample of this.#queuedChunkStarts) {
      if (startSample < firstChunk) {
        this.#queuedChunkStarts.delete(startSample);
      }
    }

    const throughSample = fromSample + LUMEN_AUDIO_LOOKAHEAD_FRAMES * this.#rate;
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
