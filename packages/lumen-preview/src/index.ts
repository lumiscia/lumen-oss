import { LumenMediaBridge } from "./media/index.js";
export { LumenAudioEngine } from "./audio-engine.js";
export {
  LumenPreviewContext,
  createLumenPreview,
  type LumenPreviewListener,
  type LumenPreviewPatch,
  type LumenPreviewState,
  type LumenPreviewTransport,
} from "./preview.js";
export type {
  AudioEngineClip,
  AudioEngineTimeline,
  AudioEngineTrack,
  AudioSourceRegistration,
} from "./audio-engine.js";
import type {
  LumenMediaTarget,
  MediaRegistration,
  MediaSourceInput,
  VideoFrameMetadata,
  VideoSourceRegistration,
} from "./media/index.js";
import { toUint8Array } from "./media/index.js";

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

export type LumenLogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";

export interface LumenMediaStoreBinding extends LumenMediaTarget {
  clear(): void;
  clearVideos(): void;
  clearVideoSource(streamId: string): void;
  hasFont(fontFamily: string): boolean;
  removeFontFamily(fontFamily: string): void;
  removeImageSource(imageId: string): void;
  removeVideoSource(streamId: string): void;
  setFont(fontFamily: string, bytes: Uint8Array): void;
}

export interface LumenRendererBinding {
  clear(): void;
  durationFrames(): number;
  frameRequirements(frame: number, media: LumenMediaStoreBinding): string;
  frameRequirementsWindow(frame: number, media: LumenMediaStoreBinding): string;
  loadComposition(compositionJson: string): void;
  renderFrame(
    frame: number,
    media: LumenMediaStoreBinding,
    canvas: HTMLCanvasElement,
  ): Promise<void>;
  setLookaheadCount(lookaheadCount: number): void;
  setLogLevel(level: LumenLogLevel): void;
}

export interface LumenPreviewControllerBinding extends LumenMediaTarget {
  clear(): void;
  clearMedia(): void;
  clearVideos(): void;
  clearVideoSource(streamId: string): void;
  currentFrame(): number;
  durationFrames(): number;
  frameRequirements(frame: number): string;
  frameRequirementsWindow(frame: number): string;
  height(): number;
  hasFont(fontFamily: string): boolean;
  isPlaying(): boolean;
  loadComposition(compositionJson: string, fps: number): void;
  pause(): void;
  play(): void;
  removeFontFamily(fontFamily: string): void;
  removeImageSource(imageId: string): void;
  removeVideoSource(streamId: string): void;
  renderNow(canvas: HTMLCanvasElement): Promise<void>;
  setFrame(frame: number): void;
  setFont(fontFamily: string, bytes: Uint8Array): void;
  setLookaheadCount(lookaheadCount: number): void;
  setLogLevel(level: LumenLogLevel): void;
  targetFrameForTimeMs(timeMs: number): number;
  tick(nowMs: number, canvas: HTMLCanvasElement): Promise<boolean>;
  width(): number;
}

export interface LumenPreviewBindings {
  LumenMediaStore: new () => LumenMediaStoreBinding;
  LumenRenderer: new () => LumenRendererBinding;
  LumenPreviewController: new () => LumenPreviewControllerBinding;
}

export interface LumenMediaStore extends LumenMediaStoreBinding {
  registerFontFamily(fontFamily: string, bytes: BufferSource): void;
  registerFontFamilyBytes(fontFamily: string, bytes: BufferSource): void;
  registerImageSource(imageId: string, source: MediaSourceInput): Promise<void>;
  registerVideoSource(
    streamId: string,
    source: MediaSourceInput,
    optionsOrFps?: number | Omit<VideoSourceRegistration, "source">,
  ): Promise<VideoFrameMetadata>;
  registerVideoSourceBytes(
    streamId: string,
    bytes: BufferSource,
    options?: Omit<VideoSourceRegistration, "source">,
  ): Promise<VideoFrameMetadata>;
  syncMediaSources(registrations: Iterable<MediaRegistration>): Promise<void>;
  loadVideoFrame(streamId: string, frame: number): Promise<void>;
  loadFrameRequirements(requirementsJson: string): Promise<void>;
}

export interface LumenRenderer extends LumenRendererBinding {
  renderFrame(frame: number, media: LumenMediaStore, canvas: HTMLCanvasElement): Promise<void>;
  renderFrameAsync(frame: number, media: LumenMediaStore, canvas: HTMLCanvasElement): Promise<void>;
}

export interface LumenPreviewController extends LumenPreviewControllerBinding {
  registerFontFamily(fontFamily: string, bytes: BufferSource): void;
  registerFontFamilyBytes(fontFamily: string, bytes: BufferSource): void;
  registerImageSource(imageId: string, source: MediaSourceInput): Promise<void>;
  registerVideoSource(
    streamId: string,
    source: MediaSourceInput,
    optionsOrFps?: number | Omit<VideoSourceRegistration, "source">,
  ): Promise<VideoFrameMetadata>;
  registerVideoSourceBytes(
    streamId: string,
    bytes: BufferSource,
    options?: Omit<VideoSourceRegistration, "source">,
  ): Promise<VideoFrameMetadata>;
  syncMediaSources(registrations: Iterable<MediaRegistration>): Promise<void>;
  loadVideoFrame(streamId: string, frame: number): Promise<void>;
  loadFrameRequirements(requirementsJson: string): Promise<void>;
  renderNowAsync(canvas: HTMLCanvasElement): Promise<void>;
  setLogLevel(level: LumenLogLevel): void;
  tickAsync(nowMs: number, canvas: HTMLCanvasElement): Promise<boolean>;
}

export interface LumenPreviewRuntime {
  LumenMediaStore: new () => LumenMediaStore;
  LumenRenderer: new () => LumenRenderer;
  LumenPreviewController: new () => LumenPreviewController;
}

export function createLumenPreviewRuntime(bindings: LumenPreviewBindings): LumenPreviewRuntime {
  class RuntimeLumenMediaStore extends bindings.LumenMediaStore implements LumenMediaStore {
    private readonly bridge = new LumenMediaBridge(this);

    clear(): void {
      super.clear();
      this.bridge.clear();
    }

    clearVideos(): void {
      super.clearVideos();
      this.bridge.clearVideos();
    }

    clearVideoSource(streamId: string): void {
      super.clearVideoSource(streamId);
      this.bridge.clearVideoSource(streamId);
    }

    registerFontFamily(fontFamily: string, bytes: BufferSource): void {
      super.setFont(fontFamily, toUint8Array(bytes));
    }

    registerFontFamilyBytes(fontFamily: string, bytes: BufferSource): void {
      this.registerFontFamily(fontFamily, bytes);
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

  class RuntimeLumenRenderer extends bindings.LumenRenderer implements LumenRenderer {
    private lookaheadCount = 8;
    private loadGeneration = 0;
    private readonly pendingWindows = new Map<number, Promise<void>>();

    clear(): void {
      super.clear();
      this.resetWindowLoads();
    }

    loadComposition(compositionJson: string): void {
      super.loadComposition(compositionJson);
      this.resetWindowLoads();
    }

    renderFrame(frame: number, media: LumenMediaStore, canvas: HTMLCanvasElement): Promise<void> {
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

  class RuntimeLumenPreviewController
    extends bindings.LumenPreviewController
    implements LumenPreviewController
  {
    private readonly bridge = new LumenMediaBridge(this);
    private lookaheadCount = 8;
    private loadGeneration = 0;
    private readonly pendingWindows = new Map<number, Promise<void>>();

    clear(): void {
      super.clear();
      this.bridge.clear();
      this.resetWindowLoads();
    }

    loadComposition(compositionJson: string, fps: number): void {
      super.loadComposition(compositionJson, fps);
      this.resetWindowLoads();
    }

    currentFrame(): number {
      return super.currentFrame();
    }

    durationFrames(): number {
      return super.durationFrames();
    }

    height(): number {
      return super.height();
    }

    isPlaying(): boolean {
      return super.isPlaying();
    }

    pause(): void {
      super.pause();
    }

    play(): void {
      super.play();
    }

    setFrame(frame: number): void {
      super.setFrame(frame);
    }

    targetFrameForTimeMs(timeMs: number): number {
      return super.targetFrameForTimeMs(timeMs);
    }

    width(): number {
      return super.width();
    }

    clearMedia(): void {
      super.clearMedia();
      this.bridge.clear();
      this.resetWindowLoads();
    }

    clearVideos(): void {
      super.clearVideos();
      this.bridge.clearVideos();
      this.resetWindowLoads();
    }

    clearVideoSource(streamId: string): void {
      super.clearVideoSource(streamId);
      this.bridge.clearVideoSource(streamId);
      this.resetWindowLoads();
    }

    removeImageSource(imageId: string): void {
      super.removeImageSource(imageId);
      this.bridge.removeImageSource(imageId, false);
      this.resetWindowLoads();
    }

    removeVideoSource(streamId: string): void {
      super.removeVideoSource(streamId);
      this.bridge.removeVideoSource(streamId, false);
      this.resetWindowLoads();
    }

    registerFontFamily(fontFamily: string, bytes: BufferSource): void {
      this.resetWindowLoads();
      super.setFont(fontFamily, toUint8Array(bytes));
    }

    registerFontFamilyBytes(fontFamily: string, bytes: BufferSource): void {
      this.registerFontFamily(fontFamily, bytes);
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

    setLogLevel(level: LumenLogLevel): void {
      super.setLogLevel(level);
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

    private async loadRequirementsForFrame(
      frame: number,
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

  return {
    LumenMediaStore: RuntimeLumenMediaStore,
    LumenRenderer: RuntimeLumenRenderer,
    LumenPreviewController: RuntimeLumenPreviewController,
  };
}
