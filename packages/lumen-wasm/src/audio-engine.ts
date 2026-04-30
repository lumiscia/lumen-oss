import type { MediaSourceInput } from "./media/index.js";
import { sourceInputToBlob } from "./media/index.js";

const CLIP_EDGE_FADE_SECONDS = 0.008;

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
    const hasSolo = timeline.tracks.some((track) => track.solo);
    const tracks = new Map(timeline.tracks.map((track) => [track.id, track]));
    const startContextTime = context.currentTime;

    for (const clip of timeline.clips) {
      const track = tracks.get(clip.trackId);
      const buffer = this.#buffers.get(clip.sourceId);
      if (!track || !buffer || track.muted || (hasSolo && !track.solo)) {
        continue;
      }

      const clipEndMs = clip.startMs + clip.durationMs;
      if (clipEndMs <= fromMs) {
        continue;
      }

      const clipOffsetMs = Math.max(0, fromMs - clip.startMs);
      const delaySeconds = Math.max(0, clip.startMs - fromMs) / 1_000;
      const sourceOffsetSeconds = (clip.sourceStartMs + clipOffsetMs) / 1_000;
      const durationSeconds = (clip.durationMs - clipOffsetMs) / 1_000;
      const source = context.createBufferSource();
      const gain = context.createGain();
      const targetGain = track.volume * clip.volume;
      const startAt = startContextTime + delaySeconds;
      const fadeSeconds = Math.min(CLIP_EDGE_FADE_SECONDS, Math.max(0, durationSeconds / 4));

      source.buffer = buffer;
      gain.gain.setValueAtTime(0, startAt);
      gain.gain.linearRampToValueAtTime(targetGain, startAt + fadeSeconds);
      gain.gain.setValueAtTime(
        targetGain,
        Math.max(startAt + fadeSeconds, startAt + durationSeconds - fadeSeconds),
      );
      gain.gain.linearRampToValueAtTime(0, startAt + durationSeconds);
      source.connect(gain).connect(context.destination);
      source.start(startAt, sourceOffsetSeconds, durationSeconds);
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
      ? (this.#timeline.durationFrames / Math.max(this.#timeline.fps, 1)) * 1_000
      : Number.POSITIVE_INFINITY;
    return Math.min(
      durationMs,
      playback.startMs + (context.currentTime - playback.startContextTime) * 1_000,
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
    this.#context ??= new AudioContext({ sampleRate: 48_000 });
    return this.#context;
  }
}
