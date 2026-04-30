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
  async preloadFrame(frame: number, media: LumenMediaStore): Promise<void> {
    const requirementsJson = super.frameRequirements(frame, media);
    await media.loadFrameRequirements(requirementsJson);
  }
}

export class LumenPreviewController extends WasmLumenPreviewController {
  private readonly bridge = new LumenMediaBridge(this);

  override clear(): void {
    super.clear();
    this.bridge.clear();
  }

  override clearMedia(): void {
    super.clearMedia();
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

  async preloadFrame(frame: number): Promise<void> {
    const requirementsJson = super.frameRequirements(frame);
    await this.bridge.loadFrameRequirements(requirementsJson);
  }
}
