import type {
  CompositionSource,
  Diagnostic,
  EvaluateRequest,
  ModelInfo,
  MediaInventory,
} from "@lumiscia/lumen-types";

import { LumenMediaBridge, toUint8Array } from "./media/index.js";
import type {
  FileMediaKind,
  LumenMediaTarget,
  MediaSourceInput,
  VideoSourceRegistration,
} from "./media/index.js";
import type { PlaybackRange } from "./playback-range.js";
import { FrameRequirementLoader } from "./requirements-loader.js";
import { readWasmError } from "./wasm-error.js";

export type LumenLogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";

/** The wasm controller, whose media comes from the bridge. */
export interface LumenPreviewControllerBinding extends LumenMediaTarget {
  audioTimeline(): string | undefined;
  currentFrame(): number;
  durationFrames(): number;
  frameRequirements(frame: number): string;
  fps(): number;
  height(): number;
  loadComposition(compositionJson: string): void;
  outputHeight(): number;
  outputWidth(): number;
  renderNow(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void>;
  setFrame(frame: number): void;
  setLogLevel(level: LumenLogLevel): void;
  setRenderScale(scale: number): void;
  width(): number;
}

export interface LumenPreviewBindings {
  /**
   * Checks a composition without loading it. With `fonts`, also warns of families none of them
   * provides.
   */
  check(compositionJson: string, fonts?: string[] | null): Diagnostic[];
  /**
   * One param of a root node as a render would evaluate it at a frame, in the JSON that sets the
   * param, without rendering. A copy of an `instance_3d` is chosen with `instance`. Throws, with
   * `diagnostics`, on a composition the engine rejects, on a node or param it doesn't have, and on
   * an expression that fails, which includes one that needs media.
   */
  evaluate(composition: string | CompositionSource, request: EvaluateRequest): unknown;
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

/** The wasm controller with each kind's browser-side decode in front of it. */
export interface LumenPreviewController {
  audioTimeline(): string | undefined;
  clear(): void;
  durationFrames(): number;
  fps(): number;
  height(): number;
  loadComposition(compositionJson: string): void;
  /** The height of rendered frames: the composition's at the render scale, rounded up. */
  outputHeight(): number;
  /** The width of rendered frames: the composition's at the render scale, rounded up. */
  outputWidth(): number;
  /** Loads what the playhead's frame needs, then starts decoding the frames after it. */
  prepareFrame(): Promise<void>;
  registerFile(kind: FileMediaKind, sourceId: string, source: MediaSourceInput): Promise<void>;
  registerFontFamily(fontFamily: string, bytes: BufferSource): void;
  registerImageSource(imageId: string, source: MediaSourceInput): Promise<void>;
  registerVideoSource(
    streamId: string,
    source: MediaSourceInput,
    options: Omit<VideoSourceRegistration, "source">,
  ): Promise<void>;
  removeFile(kind: FileMediaKind, sourceId: string): void;
  removeFontFamily(fontFamily: string): void;
  removeImageSource(imageId: string): void;
  removeVideoSource(streamId: string): void;
  /** Renders a frame `prepareFrame` loaded. */
  renderPrepared(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void>;
  setFrame(frame: number): void;
  setLogLevel(level: LumenLogLevel): void;
  setLookaheadCount(lookaheadCount: number): void;
  /** Device pixels per composition pixel from the next frame, kept across loads. */
  setRenderScale(scale: number): void;
  /** Limits lookahead to the range being played. It wraps round the range only if it loops. */
  setPlaybackRange(range: PlaybackRange | null): void;
  width(): number;
}

export interface LumenPreviewRuntime {
  LumenPreviewController: new () => LumenPreviewController;
}

export function createLumenPreviewRuntime(bindings: LumenPreviewBindings): LumenPreviewRuntime {
  class RuntimeLumenPreviewController implements LumenPreviewController {
    private readonly controller = new bindings.LumenPreviewController();
    private readonly bridge = new LumenMediaBridge(this.controller);
    private readonly requirements = new FrameRequirementLoader({
      durationFrames: () => this.controller.durationFrames(),
      load: (requirementsJson) => this.bridge.loadFrameRequirements(requirementsJson),
      requirements: (frame) => this.controller.frameRequirements(frame),
    });

    audioTimeline(): string | undefined {
      return this.controller.audioTimeline();
    }

    clear(): void {
      this.controller.clear();
      this.bridge.clear();
      this.requirements.reset();
    }

    durationFrames(): number {
      return this.controller.durationFrames();
    }

    fps(): number {
      return this.controller.fps();
    }

    height(): number {
      return this.controller.height();
    }

    loadComposition(compositionJson: string): void {
      this.controller.loadComposition(compositionJson);
      this.requirements.reset();
    }

    outputHeight(): number {
      return this.controller.outputHeight();
    }

    outputWidth(): number {
      return this.controller.outputWidth();
    }

    async prepareFrame(): Promise<void> {
      const frame = this.controller.currentFrame();
      await this.requirements.loadFrame(frame);
      this.requirements.prefetchWindow(frame + 1);
    }

    async registerFile(
      kind: FileMediaKind,
      sourceId: string,
      source: MediaSourceInput,
    ): Promise<void> {
      await this.bridge.registerFile(kind, sourceId, source);
    }

    registerFontFamily(fontFamily: string, bytes: BufferSource): void {
      this.requirements.reset();
      this.controller.setFont(fontFamily, toUint8Array(bytes));
    }

    async registerImageSource(imageId: string, source: MediaSourceInput): Promise<void> {
      this.requirements.reset();
      await this.bridge.registerImageSource(imageId, source);
    }

    async registerVideoSource(
      streamId: string,
      source: MediaSourceInput,
      options: Omit<VideoSourceRegistration, "source">,
    ): Promise<void> {
      this.requirements.reset();
      await this.bridge.registerVideoSource(streamId, source, options);
    }

    removeFile(kind: FileMediaKind, sourceId: string): void {
      this.bridge.removeFile(kind, sourceId);
    }

    removeFontFamily(fontFamily: string): void {
      this.controller.removeFontFamily(fontFamily);
      this.requirements.reset();
    }

    removeImageSource(imageId: string): void {
      this.bridge.removeImageSource(imageId);
      this.requirements.reset();
    }

    removeVideoSource(streamId: string): void {
      this.bridge.removeVideoSource(streamId);
      this.requirements.reset();
    }

    async renderPrepared(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void> {
      try {
        await this.controller.renderNow(canvas);
      } catch (error) {
        if (!this.isRecoverableFrameMiss(error)) {
          throw error;
        }
        await this.requirements.loadFrame(this.controller.currentFrame());
        await this.controller.renderNow(canvas);
      }
    }

    setFrame(frame: number): void {
      this.controller.setFrame(frame);
    }

    setLogLevel(level: LumenLogLevel): void {
      this.controller.setLogLevel(level);
    }

    setLookaheadCount(lookaheadCount: number): void {
      this.requirements.setLookaheadCount(lookaheadCount);
    }

    setRenderScale(scale: number): void {
      this.controller.setRenderScale(scale);
    }

    setPlaybackRange(range: PlaybackRange | null): void {
      this.requirements.setRange(range);
    }

    width(): number {
      return this.controller.width();
    }

    private isRecoverableFrameMiss(error: unknown): boolean {
      return readWasmError(error).frameNotLoaded;
    }
  }

  return {
    LumenPreviewController: RuntimeLumenPreviewController,
  };
}
