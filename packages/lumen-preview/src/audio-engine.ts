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

export interface AudioSourceRegistration {
  id: string;
  kind: "audio";
  source: MediaSourceInput;
}

type ActivePlayback = {
  cleanup: () => void;
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

export class LumenAudioEngine {
  #buffers = new Map<string, AudioBuffer>();
  #context: AudioContext | null = null;
  #playback: ActivePlayback | null = null;
  #timeline: AudioEngineTimeline | null = null;
  #pausedMs = 0;
  #buffering = false;

  async syncAudioSources(registrations: Iterable<AudioSourceRegistration>): Promise<void> {
    const desired = new Map<string, AudioSourceRegistration>();
    for (const registration of registrations) {
      desired.set(registration.id, registration);
    }

    for (const id of this.#buffers.keys()) {
      if (!desired.has(id)) {
        this.#buffers.delete(id);
      }
    }

    for (const registration of desired.values()) {
      if (this.#buffers.has(registration.id)) {
        continue;
      }
      const blob = await sourceInputToBlob(registration.source);
      const bytes = await blob.arrayBuffer();
      const decoded = await this.#getContext().decodeAudioData(bytes);
      this.#buffers.set(registration.id, decoded);
    }
  }

  setAudioTimeline(timeline: AudioEngineTimeline | null): void {
    this.#timeline = timeline;
  }

  async preloadAudioWindow(_timeMs: number, _durationMs = 2_000): Promise<void> {
    this.#buffering = false;
  }

  play(fromMs = this.#pausedMs): void {
    this.stop();
    const timeline = this.#timeline;
    if (!timeline || timeline.clips.length === 0) {
      this.#pausedMs = fromMs;
      return;
    }

    const context = this.#getContext();
    void context.resume();
    const startedNodes: AudioBufferSourceNode[] = [];
    const startContextTime = context.currentTime;

    for (const scheduled of createLumenAudioSchedule(timeline, fromMs)) {
      const { clip, delaySeconds, durationSeconds, fadeSeconds, gain: targetGain } = scheduled;
      const buffer = this.#buffers.get(clip.sourceId);
      if (!buffer) {
        continue;
      }

      const source = context.createBufferSource();
      const gain = context.createGain();
      const startAt = startContextTime + delaySeconds;

      source.buffer = buffer;
      gain.gain.setValueAtTime(0, startAt);
      gain.gain.linearRampToValueAtTime(targetGain, startAt + fadeSeconds);
      gain.gain.setValueAtTime(
        targetGain,
        Math.max(startAt + fadeSeconds, startAt + durationSeconds - fadeSeconds),
      );
      gain.gain.linearRampToValueAtTime(0, startAt + durationSeconds);
      source.connect(gain).connect(context.destination);
      source.start(startAt, scheduled.sourceOffsetSeconds, durationSeconds);
      startedNodes.push(source);
    }

    this.#playback = {
      startContextTime,
      startMs: fromMs,
      cleanup: () => {
        for (const node of startedNodes) {
          try {
            node.stop();
          } catch {
            // Already stopped.
          }
          node.disconnect();
        }
      },
    };
    this.#pausedMs = fromMs;
  }

  pause(): void {
    this.#pausedMs = this.currentTimeMs();
    this.#playback?.cleanup();
    this.#playback = null;
  }

  stop(): void {
    this.#playback?.cleanup();
    this.#playback = null;
  }

  seekMs(ms: number): void {
    const wasPlaying = this.#playback !== null;
    this.#pausedMs = Math.max(0, ms);
    this.stop();
    if (wasPlaying) {
      this.play(this.#pausedMs);
    }
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

  dispose(): void {
    this.stop();
    this.#buffers.clear();
    void this.#context?.close();
    this.#context = null;
  }

  #getContext(): AudioContext {
    this.#context ??= new AudioContext({ sampleRate: LUMEN_AUDIO_SAMPLE_RATE });
    return this.#context;
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

export function msToLumenAudioSample(ms: number): number {
  return Math.floor((Math.max(0, ms) * LUMEN_AUDIO_SAMPLE_RATE) / MS_PER_SECOND);
}

export function lumenAudioSamplesToSeconds(samples: number): number {
  return Math.max(0, samples) / LUMEN_AUDIO_SAMPLE_RATE;
}
