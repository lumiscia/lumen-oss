import { ALL_FORMATS, type InputFormat } from "mediabunny";

import { closeVideoFrame, syncVideoFrameToTarget, takesYuvPlanes } from "./canvas.js";
import { LumenMediaError } from "./errors.js";
import { ImageSession } from "./image-session.js";
import { readModel } from "./model.js";
import {
  DEFAULT_BLOB_CACHE_BYTES,
  DEFAULT_URL_CACHE_BYTES,
  sameMediaInput,
  type MediaSourceDefaults,
} from "./source.js";
import type {
  FrameRequirementsPayload,
  LumenMediaBridgeOptions,
  MediaRegistration,
  MediaSourceInput,
  NativeVideoFrameTarget,
  RegisteredImageSource,
  RegisteredModelSource,
  RegisteredVideoSource,
  VideoFrameMetadata,
  VideoFramePayload,
  VideoSourceRegistration,
} from "./types.js";
import {
  DEFAULT_VIDEO_FRAME_CACHE_CAPACITY,
  VideoSession,
  createVideoRegistration,
  dedupeFrameNumbers,
} from "./video-session.js";

type NormalizedBridgeOptions = Required<LumenMediaBridgeOptions>;

// Async work holds on to its entry and uploads only while the map still holds that same object,
// so nothing from a removed source can land on a later source with the same id.
type ImageEntry = {
  session: ImageSession;
  source: MediaSourceInput;
};

type ModelEntry = {
  source: MediaSourceInput;
  /** The read and hand-over in flight, which loads of the same entry join. */
  upload: Promise<void> | null;
};

type VideoEntry = {
  registration: VideoSourceRegistration;
  session: VideoSession;
  syncedMetadata: VideoFrameMetadata | null;
};

export class LumenMediaBridge<TTarget extends NativeVideoFrameTarget> {
  private readonly images = new Map<string, ImageEntry>();
  private readonly models = new Map<string, ModelEntry>();
  private readonly options: NormalizedBridgeOptions;
  private syncGeneration = 0;
  private readonly videos = new Map<string, VideoEntry>();

  constructor(
    private readonly target: TTarget,
    options: LumenMediaBridgeOptions = {},
  ) {
    this.options = normalizeBridgeOptions(options);
    target.setVideoFrameCacheBytes?.(this.options.frameCacheCapacity);
  }

  clear(): void {
    this.syncGeneration += 1;
    this.images.clear();
    this.models.clear();
    for (const video of this.videos.values()) {
      video.session.dispose();
    }
    this.videos.clear();
  }

  clearVideos(): void {
    for (const video of this.videos.values()) {
      this.resetVideoEntry(video);
    }
  }

  clearVideoSource(streamId: string): void {
    const video = this.videos.get(streamId);
    if (!video) {
      return;
    }
    this.resetVideoEntry(video);
  }

  removeImageSource(imageId: string, syncTarget = true): void {
    this.images.delete(imageId);
    if (syncTarget) {
      this.target.removeImageSource?.(imageId);
    }
  }

  removeModelSource(modelId: string, syncTarget = true): void {
    this.models.delete(modelId);
    if (syncTarget) {
      this.target.removeModelSource?.(modelId);
    }
  }

  removeVideoSource(streamId: string, syncTarget = true): void {
    const entry = this.videos.get(streamId);
    entry?.session.dispose();
    this.videos.delete(streamId);
    if (syncTarget) {
      this.target.removeVideoSource?.(streamId);
    }
  }

  /** A newer sync or `clear` abandons this one, which then resolves without registering more. */
  async syncMediaSources(registrations: Iterable<MediaRegistration>): Promise<void> {
    const generation = ++this.syncGeneration;
    const desiredImages = new Map<string, RegisteredImageSource>();
    const desiredModels = new Map<string, RegisteredModelSource>();
    const desiredVideos = new Map<string, RegisteredVideoSource>();

    for (const registration of registrations) {
      if (registration.kind === "image") {
        desiredImages.set(registration.id, registration);
      } else if (registration.kind === "model") {
        desiredModels.set(registration.id, registration);
      } else {
        desiredVideos.set(registration.id, registration);
      }
    }

    for (const imageId of this.images.keys()) {
      if (!desiredImages.has(imageId)) {
        this.removeImageSource(imageId);
      }
    }

    for (const modelId of this.models.keys()) {
      if (!desiredModels.has(modelId)) {
        this.removeModelSource(modelId);
      }
    }

    for (const streamId of this.videos.keys()) {
      if (!desiredVideos.has(streamId)) {
        this.removeVideoSource(streamId);
      }
    }

    try {
      for (const image of desiredImages.values()) {
        if (generation !== this.syncGeneration) {
          return;
        }
        await this.registerImageSource(image.id, image.source);
      }

      for (const model of desiredModels.values()) {
        if (generation !== this.syncGeneration) {
          return;
        }
        await this.registerModelSource(model.id, model.source);
      }

      for (const video of desiredVideos.values()) {
        if (generation !== this.syncGeneration) {
          return;
        }
        const { id, source, ...options } = video;
        await this.registerVideoSource(id, source, options);
      }
    } catch (error) {
      if (generation === this.syncGeneration) {
        throw error;
      }
    }
  }

  async registerImageSource(imageId: string, source: MediaSourceInput): Promise<void> {
    const existing = this.images.get(imageId);
    if (existing && sameMediaInput(existing.source, source)) {
      await this.loadImageSource(imageId);
      return;
    }

    this.removeImageSource(imageId);
    this.images.set(imageId, {
      session: new ImageSession(imageId, source),
      source,
    });
    await this.loadImageSource(imageId);
  }

  async registerModelSource(modelId: string, source: MediaSourceInput): Promise<void> {
    const existing = this.models.get(modelId);
    if (existing && sameMediaInput(existing.source, source)) {
      await this.loadModelSource(modelId);
      return;
    }

    this.removeModelSource(modelId);
    this.models.set(modelId, { source, upload: null });
    await this.loadModelSource(modelId);
  }

  async registerVideoSource(
    streamId: string,
    source: MediaSourceInput,
    optionsOrFps: number | Omit<VideoSourceRegistration, "source"> = {},
  ): Promise<VideoFrameMetadata> {
    const options = typeof optionsOrFps === "number" ? { fps: optionsOrFps } : { ...optionsOrFps };
    const registration = createVideoRegistration(source, {
      formats: this.options.formats,
      ...options,
    });
    const existing = this.videos.get(streamId);
    if (existing && sameVideoRegistration(existing.registration, registration)) {
      return this.loadVideoMetadata(streamId);
    }

    this.removeVideoSource(streamId);
    this.videos.set(streamId, {
      registration,
      session: new VideoSession(streamId, registration, this.options, takesYuvPlanes(this.target)),
      syncedMetadata: null,
    });
    return this.loadVideoMetadata(streamId);
  }

  async registerVideoSourceBytes(
    streamId: string,
    bytes: BufferSource,
    options: Omit<VideoSourceRegistration, "source"> = {},
  ): Promise<VideoFrameMetadata> {
    return this.registerVideoSource(streamId, bytes, options);
  }

  async loadImageSource(imageId: string): Promise<void> {
    const entry = this.images.get(imageId);
    if (!entry) {
      throw new LumenMediaError(
        "source_not_registered",
        `image source "${imageId}" is not registered`,
        { sourceId: imageId },
      );
    }
    if (this.target.hasImage(imageId)) {
      return;
    }

    const frame = await entry.session.load();
    if (this.images.get(imageId) === entry) {
      this.target.setImage(imageId, frame.width, frame.height, frame.rgba);
    }
  }

  async loadModelSource(modelId: string): Promise<void> {
    const entry = this.models.get(modelId);
    if (!entry) {
      throw new LumenMediaError(
        "source_not_registered",
        `model source "${modelId}" is not registered`,
        { sourceId: modelId },
      );
    }
    if (this.target.hasModel?.(modelId)) {
      return;
    }

    entry.upload ??= readModel(modelId, entry.source)
      .then((bytes) => {
        if (this.models.get(modelId) === entry) {
          this.target.setModel?.(modelId, bytes);
        }
      })
      .finally(() => {
        entry.upload = null;
      });
    await entry.upload;
  }

  async loadVideoMetadata(streamId: string): Promise<VideoFrameMetadata> {
    const entry = this.requireVideoEntry(streamId);
    if (entry.syncedMetadata) {
      return entry.syncedMetadata;
    }

    const metadata = await entry.session.loadMetadata();
    if (this.videos.get(streamId) !== entry) {
      return metadata;
    }

    this.target.setVideoMetadata(
      streamId,
      metadata.width,
      metadata.height,
      metadata.frameCount,
      metadata.fps,
    );
    entry.syncedMetadata = metadata;
    return metadata;
  }

  async loadVideoFrame(streamId: string, frame: number): Promise<void> {
    await this.syncDecodedFrames(streamId, [frame]);
  }

  async loadFrameRequirements(requirements: FrameRequirementsPayload | string): Promise<void> {
    const parsed = parseFrameRequirements(requirements);

    await Promise.all(parsed.images.map((imageId) => this.loadImageSource(imageId)));
    await Promise.all(
      parsed.videos.map((video) => this.syncDecodedFrames(video.streamId, video.frames)),
    );
  }

  private requireVideoEntry(streamId: string): VideoEntry {
    const entry = this.videos.get(streamId);
    if (!entry) {
      throw new LumenMediaError(
        "source_not_registered",
        `video source "${streamId}" is not registered`,
        { sourceId: streamId },
      );
    }
    return entry;
  }

  private resetVideoEntry(entry: VideoEntry): void {
    entry.session.clearFrames();
    entry.syncedMetadata = null;
  }

  private async syncDecodedFrames(streamId: string, frames: Iterable<number>): Promise<void> {
    const entry = this.requireVideoEntry(streamId);
    const wantedFrames = dedupeFrameNumbers(frames).filter(
      (frame) => !this.target.hasVideoFrame(streamId, frame),
    );
    if (wantedFrames.length === 0) {
      return;
    }

    const isCurrent = () => this.videos.get(streamId) === entry;
    let decodedFrames: Map<number, VideoFramePayload>;
    try {
      await this.loadVideoMetadata(streamId);
      decodedFrames = await entry.session.decodeFrames(wantedFrames);
    } catch (error) {
      // Removing a source cancels its reads, which is not a failure of this load.
      if (!isCurrent()) {
        return;
      }
      throw error;
    }

    try {
      for (const [frame, payload] of decodedFrames) {
        decodedFrames.delete(frame);
        await syncVideoFrameToTarget(this.target, streamId, frame, payload, isCurrent);
      }
    } finally {
      for (const payload of decodedFrames.values()) {
        closeVideoFrame(payload.videoFrame);
      }
    }
  }
}

export function parseFrameRequirements(
  requirements: FrameRequirementsPayload | string,
): FrameRequirementsPayload {
  return typeof requirements === "string"
    ? (JSON.parse(requirements) as FrameRequirementsPayload)
    : requirements;
}

function normalizeBridgeOptions(options: LumenMediaBridgeOptions): NormalizedBridgeOptions {
  return {
    blobCacheBytes: options.blobCacheBytes ?? DEFAULT_BLOB_CACHE_BYTES,
    frameCacheCapacity: options.frameCacheCapacity ?? DEFAULT_VIDEO_FRAME_CACHE_CAPACITY,
    formats: options.formats ?? ALL_FORMATS,
    urlCacheBytes: options.urlCacheBytes ?? DEFAULT_URL_CACHE_BYTES,
  };
}

function sameVideoRegistration(
  left: VideoSourceRegistration,
  right: VideoSourceRegistration,
): boolean {
  return (
    sameMediaInput(left.source, right.source) &&
    left.fps === right.fps &&
    left.track === right.track &&
    sameFormats(left.formats, right.formats)
  );
}

function sameFormats(left: InputFormat[] | undefined, right: InputFormat[] | undefined): boolean {
  if (left === right) {
    return true;
  }
  if (!left || !right || left.length !== right.length) {
    return false;
  }
  return left.every((format, index) => format === right[index]);
}

export type { MediaSourceDefaults };
