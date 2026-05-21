import { LumenMediaBridge } from "./media/index.js";
export {
  audioTimelineFromCompositionJson,
  createLumenAudioSchedule,
  defaultAudioWorkerUrl,
  defaultAudioWorkletUrl,
  lumenAudioSamplesToSeconds,
  LumenAudioEngine,
  msToLumenAudioSample,
} from "./audio-engine.js";
export {
  LumenPreviewContext,
  createLumenPreview,
  type LumenPreviewListener,
  type LumenPreviewPatch,
  type LumenPreviewState,
  type LumenPreviewTransport,
} from "./preview.js";
export { LumenPreviewSession } from "./session/index.js";
export type {
  LumenPreviewSessionInputs,
  LumenPreviewSessionOptions,
  LumenPreviewStats,
  LumenPreviewStatsCallback,
} from "./session/index.js";
export type {
  AudioEngineClip,
  AudioEngineTimeline,
  AudioEngineTrack,
  LumenAudioEngineOptions,
  AudioSourceRegistration,
  ScheduledAudioClip,
} from "./audio-engine.js";
import type {
  LumenMediaTarget,
  MediaRegistration,
  MediaSourceInput,
  VideoFrameMetadata,
  VideoSourceRegistration,
} from "./media/index.js";
import { toUint8Array } from "./media/index.js";
import { FrameRequirementLoader } from "./requirements-loader.js";

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
  renderFrameToOffscreenCanvas(
    frame: number,
    media: LumenMediaStoreBinding,
    canvas: OffscreenCanvas,
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
  fps(): number;
  targetFrameDurationMs(): number;
  height(): number;
  hasFont(fontFamily: string): boolean;
  isPlaying(): boolean;
  loadComposition(compositionJson: string): void;
  pause(): void;
  play(): void;
  removeFontFamily(fontFamily: string): void;
  removeImageSource(imageId: string): void;
  removeVideoSource(streamId: string): void;
  renderNow(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void>;
  setFrame(frame: number): void;
  setFont(fontFamily: string, bytes: Uint8Array): void;
  setLookaheadCount(lookaheadCount: number): void;
  setLogLevel(level: LumenLogLevel): void;
  targetFrameForTimeMs(timeMs: number): number;
  tick(nowMs: number, canvas: HTMLCanvasElement | OffscreenCanvas): Promise<boolean>;
  width(): number;
}

export interface LumenPreviewBindings {
  LumenMediaStore: new () => LumenMediaStoreBinding;
  LumenRenderer: new () => LumenRendererBinding;
  LumenPreviewController: new () => LumenPreviewControllerBinding;
}

export interface LumenBindings {
  readonly target?: string;
  previewWorkerUrl?: () => string | URL;
  preview: () => Promise<LumenPreviewBindings>;
}

export type LumenPreviewBindingSource = LumenPreviewBindings | LumenBindings;

export async function resolveLumenPreviewBindings(
  bindings: LumenPreviewBindingSource,
): Promise<LumenPreviewBindings> {
  if (isLumenBindings(bindings)) {
    return bindings.preview();
  }

  return bindings;
}

function isLumenBindings(bindings: LumenPreviewBindingSource): bindings is LumenBindings {
  return typeof (bindings as LumenBindings).preview === "function";
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
  renderFrame(
    frame: number,
    media: LumenMediaStore,
    canvas: HTMLCanvasElement | OffscreenCanvas,
  ): Promise<void>;
  renderFrameAsync(
    frame: number,
    media: LumenMediaStore,
    canvas: HTMLCanvasElement | OffscreenCanvas,
  ): Promise<void>;
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
  renderNowAsync(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void>;
  setLogLevel(level: LumenLogLevel): void;
  tickAsync(nowMs: number, canvas: HTMLCanvasElement | OffscreenCanvas): Promise<boolean>;
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
    private readonly requirements: FrameRequirementLoader;

    constructor() {
      super();
      this.requirements = new FrameRequirementLoader({
        durationFrames: () => super.durationFrames(),
        load: (requirementsJson) => this.requireMedia().loadFrameRequirements(requirementsJson),
        requirements: (frame) => super.frameRequirements(frame, this.requireMedia()),
      });
    }

    private media: LumenMediaStore | null = null;

    clear(): void {
      super.clear();
      this.requirements.reset();
    }

    loadComposition(compositionJson: string): void {
      super.loadComposition(compositionJson);
      this.requirements.reset();
    }

    renderFrame(
      frame: number,
      media: LumenMediaStore,
      canvas: HTMLCanvasElement | OffscreenCanvas,
    ): Promise<void> {
      if (isHtmlCanvas(canvas)) {
        return super.renderFrame(frame, media, canvas);
      }
      return super.renderFrameToOffscreenCanvas(frame, media, canvas);
    }

    setLookaheadCount(lookaheadCount: number): void {
      super.setLookaheadCount(lookaheadCount);
      this.requirements.setLookaheadCount(lookaheadCount);
    }

    async renderFrameAsync(
      frame: number,
      media: LumenMediaStore,
      canvas: HTMLCanvasElement | OffscreenCanvas,
    ): Promise<void> {
      if (this.media !== media) {
        this.media = media;
        this.requirements.reset();
      }
      await this.requirements.loadFrame(frame);
      await this.renderFrame(frame, media, canvas);
      this.requirements.prefetchWindow(frame + 1);
    }

    private requireMedia(): LumenMediaStore {
      if (!this.media) {
        throw new Error("LumenRenderer media store is not attached");
      }
      return this.media;
    }
  }

  class RuntimeLumenPreviewController
    extends bindings.LumenPreviewController
    implements LumenPreviewController
  {
    private readonly bridge = new LumenMediaBridge(this);
    private readonly requirements: FrameRequirementLoader;

    constructor() {
      super();
      this.requirements = new FrameRequirementLoader({
        durationFrames: () => super.durationFrames(),
        load: (requirementsJson) => this.bridge.loadFrameRequirements(requirementsJson),
        requirements: (frame) => super.frameRequirements(frame),
      });
    }

    clear(): void {
      super.clear();
      this.bridge.clear();
      this.requirements.reset();
    }

    loadComposition(compositionJson: string): void {
      super.loadComposition(compositionJson);
      this.requirements.reset();
    }

    clearMedia(): void {
      super.clearMedia();
      this.bridge.clear();
      this.requirements.reset();
    }

    clearVideos(): void {
      super.clearVideos();
      this.bridge.clearVideos();
      this.requirements.reset();
    }

    clearVideoSource(streamId: string): void {
      super.clearVideoSource(streamId);
      this.bridge.clearVideoSource(streamId);
      this.requirements.reset();
    }

    removeImageSource(imageId: string): void {
      super.removeImageSource(imageId);
      this.bridge.removeImageSource(imageId, false);
      this.requirements.reset();
    }

    removeVideoSource(streamId: string): void {
      super.removeVideoSource(streamId);
      this.bridge.removeVideoSource(streamId, false);
      this.requirements.reset();
    }

    registerFontFamily(fontFamily: string, bytes: BufferSource): void {
      this.requirements.reset();
      super.setFont(fontFamily, toUint8Array(bytes));
    }

    registerFontFamilyBytes(fontFamily: string, bytes: BufferSource): void {
      this.registerFontFamily(fontFamily, bytes);
    }

    async registerImageSource(imageId: string, source: MediaSourceInput): Promise<void> {
      this.requirements.reset();
      await this.bridge.registerImageSource(imageId, source);
    }

    async registerVideoSource(
      streamId: string,
      source: MediaSourceInput,
      optionsOrFps: number | Omit<VideoSourceRegistration, "source"> = {},
    ): Promise<VideoFrameMetadata> {
      this.requirements.reset();
      return this.bridge.registerVideoSource(streamId, source, optionsOrFps);
    }

    async registerVideoSourceBytes(
      streamId: string,
      bytes: BufferSource,
      options: Omit<VideoSourceRegistration, "source"> = {},
    ): Promise<VideoFrameMetadata> {
      this.requirements.reset();
      return this.bridge.registerVideoSourceBytes(streamId, bytes, options);
    }

    async syncMediaSources(registrations: Iterable<MediaRegistration>): Promise<void> {
      this.requirements.reset();
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
      this.requirements.setLookaheadCount(lookaheadCount);
    }

    async renderNowAsync(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void> {
      await this.requirements.loadFrame(super.currentFrame());
      try {
        await super.renderNow(canvas);
      } catch (error) {
        if (!this.isRecoverableFrameMiss(error)) {
          throw error;
        }
        await this.requirements.loadFrame(super.currentFrame());
        await super.renderNow(canvas);
      }
      this.requirements.prefetchWindow(super.currentFrame() + 1);
    }

    async tickAsync(nowMs: number, canvas: HTMLCanvasElement | OffscreenCanvas): Promise<boolean> {
      const frame = super.currentFrame();
      this.requirements.prefetchWindow(frame + 1);
      await this.requirements.loadFrame(frame);
      let changed: boolean;
      try {
        changed = await super.tick(nowMs, canvas);
      } catch (error) {
        if (!this.isRecoverableFrameMiss(error)) {
          throw error;
        }
        await this.requirements.loadFrame(super.currentFrame());
        await super.renderNow(canvas);
        changed = true;
      }
      this.requirements.prefetchWindow(super.currentFrame() + 1);
      return changed;
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

function isHtmlCanvas(canvas: HTMLCanvasElement | OffscreenCanvas): canvas is HTMLCanvasElement {
  return typeof HTMLCanvasElement !== "undefined" && canvas instanceof HTMLCanvasElement;
}
