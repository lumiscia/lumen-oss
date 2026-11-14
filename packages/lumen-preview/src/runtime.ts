import type { Diagnostic, ModelInfo, MediaInventory } from "@lumiscia/lumen-types";

import { LumenMediaBridge, toUint8Array } from "./media/index.js";
import type { LumenMediaTarget, MediaSourceInput, VideoSourceRegistration } from "./media/index.js";
import type { PlaybackRange } from "./playback-range.js";
import { FrameRequirementLoader } from "./requirements-loader.js";

export type LumenLogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";

export interface LumenPreviewControllerBinding extends LumenMediaTarget {
  audioTimeline(): string | undefined;
  clear(): void;
  clearMedia(): void;
  clearVideos(): void;
  clearVideoSource(streamId: string): void;
  currentFrame(): number;
  durationFrames(): number;
  frameRequirements(frame: number): string;
  fps(): number;
  height(): number;
  hasEnvironment(sourceId: string): boolean;
  hasFont(fontFamily: string): boolean;
  hasLut(sourceId: string): boolean;
  hasModel(modelId: string): boolean;
  loadComposition(compositionJson: string): void;
  removeEnvironmentSource(sourceId: string): void;
  removeFontFamily(fontFamily: string): void;
  removeImageSource(imageId: string): void;
  removeLutSource(sourceId: string): void;
  removeModelSource(modelId: string): void;
  removeVideoSource(streamId: string): void;
  renderNow(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void>;
  setEnvironment(sourceId: string, bytes: Uint8Array): void;
  setFrame(frame: number): void;
  setFont(fontFamily: string, bytes: Uint8Array): void;
  setLut(sourceId: string, bytes: Uint8Array): void;
  setModel(modelId: string, bytes: Uint8Array): void;
  setLogLevel(level: LumenLogLevel): void;
  /** Throws as `loadComposition` would on a composition the engine rejects, without loading it. */
  validateComposition(compositionJson: string): void;
  width(): number;
}

export interface LumenPreviewBindings {
  /**
   * Checks a composition without loading it. With `fonts`, also warns of families none of them
   * provides.
   */
  check(compositionJson: string, fonts?: string[] | null): Diagnostic[];
  /**
   * What a composition needs before it renders: the media its nodes and audio clips name, with
   * the kinds each reads and where it is used, and the font families its text uses. Throws, with
   * `diagnostics`, on a composition the engine rejects.
   */
  mediaDependencies(compositionJson: string): MediaInventory;
  /**
   * The animation clips and materials of a binary glTF (`.glb`), which a `model_3d` selects with
   * `animation` and `texture_material`. Reads the file as a render would and throws on one it
   * cannot read.
   */
  modelInfo(bytes: Uint8Array): ModelInfo;
  LumenPreviewController: new () => LumenPreviewControllerBinding;
}

export interface LumenBindings {
  preview: () => Promise<LumenPreviewBindings>;
}

/** The binding with each kind's browser-side decode in front of it. */
export interface LumenPreviewController extends LumenPreviewControllerBinding {
  registerEnvironmentSource(sourceId: string, source: MediaSourceInput): Promise<void>;
  registerFontFamily(fontFamily: string, bytes: BufferSource): void;
  registerImageSource(imageId: string, source: MediaSourceInput): Promise<void>;
  registerLutSource(sourceId: string, source: MediaSourceInput): Promise<void>;
  registerModelSource(modelId: string, source: MediaSourceInput): Promise<void>;
  registerVideoSource(
    streamId: string,
    source: MediaSourceInput,
    options: Omit<VideoSourceRegistration, "source">,
  ): Promise<void>;
  /** Limits lookahead to the range being played. It wraps round the range only if it loops. */
  setPlaybackRange(range: PlaybackRange | null): void;
  /** Loads what the playhead's frame needs, then starts decoding the frames after it. */
  prepareFrame(): Promise<void>;
  /** Renders a frame `prepareFrame` loaded. */
  renderPrepared(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void>;
  setLookaheadCount(lookaheadCount: number): void;
  setLogLevel(level: LumenLogLevel): void;
}

export interface LumenPreviewRuntime {
  LumenPreviewController: new () => LumenPreviewController;
}

export function createLumenPreviewRuntime(bindings: LumenPreviewBindings): LumenPreviewRuntime {
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

    removeModelSource(modelId: string): void {
      super.removeModelSource(modelId);
      this.bridge.removeModelSource(modelId, false);
    }

    removeEnvironmentSource(sourceId: string): void {
      super.removeEnvironmentSource(sourceId);
      this.bridge.removeEnvironmentSource(sourceId, false);
    }

    removeLutSource(sourceId: string): void {
      super.removeLutSource(sourceId);
      this.bridge.removeLutSource(sourceId, false);
    }

    removeFontFamily(fontFamily: string): void {
      super.removeFontFamily(fontFamily);
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

    async registerImageSource(imageId: string, source: MediaSourceInput): Promise<void> {
      this.requirements.reset();
      await this.bridge.registerImageSource(imageId, source);
    }

    async registerModelSource(modelId: string, source: MediaSourceInput): Promise<void> {
      await this.bridge.registerModelSource(modelId, source);
    }

    async registerEnvironmentSource(sourceId: string, source: MediaSourceInput): Promise<void> {
      await this.bridge.registerEnvironmentSource(sourceId, source);
    }

    async registerLutSource(sourceId: string, source: MediaSourceInput): Promise<void> {
      await this.bridge.registerLutSource(sourceId, source);
    }

    async registerVideoSource(
      streamId: string,
      source: MediaSourceInput,
      options: Omit<VideoSourceRegistration, "source">,
    ): Promise<void> {
      this.requirements.reset();
      await this.bridge.registerVideoSource(streamId, source, options);
    }

    setLookaheadCount(lookaheadCount: number): void {
      this.requirements.setLookaheadCount(lookaheadCount);
    }

    setPlaybackRange(range: PlaybackRange | null): void {
      this.requirements.setRange(range);
    }

    async prepareFrame(): Promise<void> {
      const frame = super.currentFrame();
      await this.requirements.loadFrame(frame);
      this.requirements.prefetchWindow(frame + 1);
    }

    async renderPrepared(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void> {
      try {
        await super.renderNow(canvas);
      } catch (error) {
        if (!this.isRecoverableFrameMiss(error)) {
          throw error;
        }
        await this.requirements.loadFrame(super.currentFrame());
        await super.renderNow(canvas);
      }
    }

    private isRecoverableFrameMiss(error: unknown): boolean {
      return (error as { code?: unknown } | null)?.code === "media_frame_out_of_range";
    }
  }

  return {
    LumenPreviewController: RuntimeLumenPreviewController,
  };
}
