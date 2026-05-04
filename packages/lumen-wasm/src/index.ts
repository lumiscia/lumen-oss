import {
  LumenMediaStore as WasmLumenMediaStore,
  LumenPreviewController as WasmLumenPreviewController,
  LumenRenderer as WasmLumenRenderer,
} from "./internal/lumen_wasm.js";

import { LumenMediaBridge } from "./media/index.js";
export { LumenAudioEngine } from "./audio-engine.js";
export type {
  AudioEngineClip,
  AudioEngineTimeline,
  AudioEngineTrack,
  AudioSourceRegistration,
} from "./audio-engine.js";
import type {
  MediaRegistration,
  MediaSourceInput,
  VideoFrameMetadata,
  VideoSourceRegistration,
} from "./media/index.js";

export type {
  BlobMediaSourceInput,
  BufferMediaSourceInput,
  DecodedImageFrame,
  FrameRequirementsPayload,
  FrameRequirementsVideoPayload,
  ImageSourceRegistration,
  LumenMediaBridgeOptions,
  LumenMediaTarget,
  MediaRegistration,
  MediaSourceInput,
  NativeVideoFrameTarget,
  RegisteredImageSource,
  RegisteredVideoSource,
  SourceMediaSourceInput,
  UrlMediaSourceInput,
  VideoFrameMetadata,
  VideoFramePayload,
  VideoFramePixels,
  VideoTimelineMode,
  VideoSourceRegistration,
} from "./media/index.js";
export {
  DEFAULT_BLOB_CACHE_BYTES,
  DEFAULT_URL_CACHE_BYTES,
  DEFAULT_VIDEO_FRAME_CACHE_CAPACITY,
  createVideoFramePayload,
  decodeVideoFramePixels,
  dedupeFrameNumbers,
  estimateVideoFrameCount,
  fetchBlob,
  normalizeFps,
  parseFrameRequirements,
  syncVideoFrameToTarget,
  toUint8Array,
} from "./media/index.js";
export { LumenMediaBridge } from "./media/index.js";

export class LumenMediaStore extends WasmLumenMediaStore {
  private readonly bridge = new LumenMediaBridge(this);

  override clear(): void {
    super.clear();
    this.bridge.clear();
  }

  override clearVideos(): void {
    super.clearVideos();
    this.bridge.clearVideos();
  }

  override clearVideoSource(streamId: string): void {
    super.clearVideoSource(streamId);
    this.bridge.clearVideoSource(streamId);
  }

  async registerImageSource(imageId: string, source: MediaSourceInput): Promise<void> {
    await this.bridge.registerImageSource(imageId, source);
  }

  async registerVideoSource(
    streamId: string,
    source: MediaSourceInput,
    optionsOrFps: number | Omit<VideoSourceRegistration, "source"> = {},
  ): Promise<VideoFrameMetadata> {
    return this.bridge.registerVideoSource(streamId, source, optionsOrFps);
  }

  async registerVideoSourceBytes(
    streamId: string,
    bytes: BufferSource,
    options: Omit<VideoSourceRegistration, "source"> = {},
  ): Promise<VideoFrameMetadata> {
    return this.bridge.registerVideoSourceBytes(streamId, bytes, options);
  }

  async syncMediaSources(registrations: Iterable<MediaRegistration>): Promise<void> {
    await this.bridge.syncMediaSources(registrations);
  }

  async loadVideoFrame(streamId: string, frame: number): Promise<void> {
    await this.bridge.loadVideoFrame(streamId, frame);
  }

  async loadFrameRequirements(requirementsJson: string): Promise<void> {
    await this.bridge.loadFrameRequirements(requirementsJson);
  }
}

export class LumenRenderer extends WasmLumenRenderer {
  private lookaheadCount = 8;
  private loadGeneration = 0;
  private readonly pendingWindows = new Map<number, Promise<void>>();

  override clear(): void {
    super.clear();
    this.resetWindowLoads();
  }

  override loadComposition(compositionJson: string): void {
    super.loadComposition(compositionJson);
    this.resetWindowLoads();
  }

  override renderFrame(
    frame: number,
    media: LumenMediaStore,
    canvas: HTMLCanvasElement,
  ): Promise<void> {
    return super.renderFrame(frame, media, canvas);
  }

  setLookaheadCount(lookaheadCount: number): void {
    super.setLookaheadCount(lookaheadCount);
    this.lookaheadCount = Math.max(0, Math.floor(lookaheadCount));
    this.resetWindowLoads();
  }

  async renderFrameAsync(
    frame: number,
    media: LumenMediaStore,
    canvas: HTMLCanvasElement,
  ): Promise<void> {
    await this.loadFrame(frame, media);
    await super.renderFrame(frame, media, canvas);
    this.prefetchWindow(frame + 1, media);
  }

  private resetWindowLoads(): void {
    this.loadGeneration += 1;
    this.pendingWindows.clear();
  }

  private normalizeFrame(frame: number): number {
    const totalFrames = super.durationFrames();
    if (totalFrames <= 0) {
      return frame;
    }

    return ((frame % totalFrames) + totalFrames) % totalFrames;
  }

  private async loadWindow(frame: number, media: LumenMediaStore): Promise<void> {
    await this.loadRequirementsForFrame(frame, media, true);
  }

  private async loadFrame(frame: number, media: LumenMediaStore): Promise<void> {
    await this.loadRequirementsForFrame(frame, media, false);
  }

  private async loadRequirementsForFrame(
    frame: number,
    media: LumenMediaStore,
    includeLookahead: boolean,
  ): Promise<void> {
    const normalizedFrame = this.normalizeFrame(frame);
    const key = includeLookahead ? normalizedFrame : -normalizedFrame - 1;
    const existing = this.pendingWindows.get(key);
    if (existing) {
      await existing;
      return;
    }

    const generation = this.loadGeneration;
    const pending = (async () => {
      const requirementsJson = includeLookahead
        ? super.frameRequirementsWindow(normalizedFrame, media)
        : super.frameRequirements(normalizedFrame, media);
      if (generation !== this.loadGeneration) {
        return;
      }
      await media.loadFrameRequirements(requirementsJson);
    })().finally(() => {
      if (this.pendingWindows.get(key) === pending) {
        this.pendingWindows.delete(key);
      }
    });
    this.pendingWindows.set(key, pending);

    try {
      await pending;
    } catch (error) {
      if (generation !== this.loadGeneration) {
        return;
      }
      throw error;
    }
  }

  private prefetchWindow(frame: number, media: LumenMediaStore): void {
    if (super.durationFrames() <= 0) {
      return;
    }

    void this.prefetchFrames(frame, media).catch(() => {
      // The foreground render path reports persistent media errors.
    });
  }

  private async prefetchFrames(frame: number, media: LumenMediaStore): Promise<void> {
    const generation = this.loadGeneration;
    for (let offset = 0; offset < this.lookaheadCount; offset += 1) {
      if (generation !== this.loadGeneration) {
        return;
      }
      await this.loadFrame(frame + offset, media);
    }
  }
}

export class LumenPreviewController extends WasmLumenPreviewController {
  private readonly bridge = new LumenMediaBridge(this);
  private lookaheadCount = 8;
  private loadGeneration = 0;
  private readonly pendingWindows = new Map<number, Promise<void>>();

  override clear(): void {
    super.clear();
    this.bridge.clear();
    this.resetWindowLoads();
  }

  override loadComposition(compositionJson: string, fps: number): void {
    super.loadComposition(compositionJson, fps);
    this.resetWindowLoads();
  }

  override clearMedia(): void {
    super.clearMedia();
    this.bridge.clear();
    this.resetWindowLoads();
  }

  override clearVideos(): void {
    super.clearVideos();
    this.bridge.clearVideos();
    this.resetWindowLoads();
  }

  override clearVideoSource(streamId: string): void {
    super.clearVideoSource(streamId);
    this.bridge.clearVideoSource(streamId);
    this.resetWindowLoads();
  }

  override removeImageSource(imageId: string): void {
    super.removeImageSource(imageId);
    this.bridge.removeImageSource(imageId, false);
    this.resetWindowLoads();
  }

  override removeVideoSource(streamId: string): void {
    super.removeVideoSource(streamId);
    this.bridge.removeVideoSource(streamId, false);
    this.resetWindowLoads();
  }

  async registerImageSource(imageId: string, source: MediaSourceInput): Promise<void> {
    this.resetWindowLoads();
    await this.bridge.registerImageSource(imageId, source);
  }

  async registerVideoSource(
    streamId: string,
    source: MediaSourceInput,
    optionsOrFps: number | Omit<VideoSourceRegistration, "source"> = {},
  ): Promise<VideoFrameMetadata> {
    this.resetWindowLoads();
    return this.bridge.registerVideoSource(streamId, source, optionsOrFps);
  }

  async registerVideoSourceBytes(
    streamId: string,
    bytes: BufferSource,
    options: Omit<VideoSourceRegistration, "source"> = {},
  ): Promise<VideoFrameMetadata> {
    this.resetWindowLoads();
    return this.bridge.registerVideoSourceBytes(streamId, bytes, options);
  }

  async syncMediaSources(registrations: Iterable<MediaRegistration>): Promise<void> {
    this.resetWindowLoads();
    await this.bridge.syncMediaSources(registrations);
  }

  async loadVideoFrame(streamId: string, frame: number): Promise<void> {
    await this.bridge.loadVideoFrame(streamId, frame);
  }

  async loadFrameRequirements(requirementsJson: string): Promise<void> {
    await this.bridge.loadFrameRequirements(requirementsJson);
  }

  setLookaheadCount(lookaheadCount: number): void {
    super.setLookaheadCount(lookaheadCount);
    this.lookaheadCount = Math.max(0, Math.floor(lookaheadCount));
    this.resetWindowLoads();
  }

  async renderNowAsync(canvas: HTMLCanvasElement): Promise<void> {
    await this.loadFrame(super.currentFrame());
    try {
      await super.renderNow(canvas);
    } catch (error) {
      if (!this.isRecoverableFrameMiss(error)) {
        throw error;
      }
      await this.loadFrame(super.currentFrame());
      await super.renderNow(canvas);
    }
    this.prefetchWindow(super.currentFrame() + 1);
  }

  async tickAsync(nowMs: number, canvas: HTMLCanvasElement): Promise<boolean> {
    const frame = super.currentFrame();
    this.prefetchWindow(frame + 1);
    await this.loadFrame(frame);
    let changed: boolean;
    try {
      changed = await super.tick(nowMs, canvas);
    } catch (error) {
      if (!this.isRecoverableFrameMiss(error)) {
        throw error;
      }
      await this.loadFrame(super.currentFrame());
      await super.renderNow(canvas);
      changed = true;
    }
    this.prefetchWindow(super.currentFrame() + 1);
    return changed;
  }

  private resetWindowLoads(): void {
    this.loadGeneration += 1;
    this.pendingWindows.clear();
  }

  private normalizeFrame(frame: number): number {
    const totalFrames = super.durationFrames();
    if (totalFrames <= 0) {
      return frame;
    }

    return ((frame % totalFrames) + totalFrames) % totalFrames;
  }

  private async loadWindow(frame: number): Promise<void> {
    await this.loadRequirementsForFrame(frame, true);
  }

  private async loadFrame(frame: number): Promise<void> {
    await this.loadRequirementsForFrame(frame, false);
  }

  private async loadRequirementsForFrame(frame: number, includeLookahead: boolean): Promise<void> {
    const normalizedFrame = this.normalizeFrame(frame);
    const key = includeLookahead ? normalizedFrame : -normalizedFrame - 1;
    const existing = this.pendingWindows.get(key);
    if (existing) {
      await existing;
      return;
    }

    const generation = this.loadGeneration;
    const pending = (async () => {
      const requirementsJson = includeLookahead
        ? super.frameRequirementsWindow(normalizedFrame)
        : super.frameRequirements(normalizedFrame);
      if (generation !== this.loadGeneration) {
        return;
      }
      await this.bridge.loadFrameRequirements(requirementsJson);
    })().finally(() => {
      if (this.pendingWindows.get(key) === pending) {
        this.pendingWindows.delete(key);
      }
    });
    this.pendingWindows.set(key, pending);

    try {
      await pending;
    } catch (error) {
      if (generation !== this.loadGeneration) {
        return;
      }
      throw error;
    }
  }

  private prefetchWindow(frame: number): void {
    if (super.durationFrames() <= 0) {
      return;
    }

    void this.prefetchFrames(frame).catch(() => {
      // The foreground render path reports persistent media errors.
    });
  }

  private async prefetchFrames(frame: number): Promise<void> {
    const generation = this.loadGeneration;
    for (let offset = 0; offset < this.lookaheadCount; offset += 1) {
      if (generation !== this.loadGeneration) {
        return;
      }
      await this.loadFrame(frame + offset);
    }
  }

  private isRecoverableFrameMiss(error: unknown): boolean {
    const message = error instanceof Error ? error.message : String(error);
    return message.includes("media frame") && message.includes("out of range");
  }
}
