import { ALL_FORMATS, type InputFormat } from "mediabunny";

import { closeVideoFrame, syncVideoFrameToTarget } from "./canvas.js";
import { LumenMediaError } from "./errors.js";
import { ImageSession } from "./image-session.js";
import {
  DEFAULT_BLOB_CACHE_BYTES,
  DEFAULT_URL_CACHE_BYTES,
  type MediaSourceDefaults,
} from "./source.js";
import type {
  FrameRequirementsPayload,
  LumenMediaBridgeOptions,
  MediaRegistration,
  MediaSourceInput,
  NativeVideoFrameTarget,
  RegisteredImageSource,
  RegisteredVideoSource,
  VideoFrameMetadata,
  VideoSourceRegistration,
} from "./types.js";
import {
  DEFAULT_VIDEO_FRAME_CACHE_CAPACITY,
  VideoSession,
  createVideoRegistration,
  dedupeFrameNumbers,
} from "./video-session.js";

type NormalizedBridgeOptions = Required<LumenMediaBridgeOptions>;

type ImageEntry = {
  session: ImageSession;
  source: MediaSourceInput;
  syncedVersion: number;
  version: number;
};

type VideoEntry = {
  registration: VideoSourceRegistration;
  session: VideoSession;
  syncedFrames: Set<number>;
  syncedMetadata: VideoFrameMetadata | null;
  syncedMetadataVersion: number;
  version: number;
};

export class LumenMediaBridge<TTarget extends NativeVideoFrameTarget> {
  private readonly images = new Map<string, ImageEntry>();
  private readonly options: NormalizedBridgeOptions;
  private readonly videos = new Map<string, VideoEntry>();

  constructor(
    private readonly target: TTarget,
    options: LumenMediaBridgeOptions = {},
  ) {
    this.options = normalizeBridgeOptions(options);
  }

  clear(): void {
    this.images.clear();
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

  removeImageSource(imageId: string): void {
    this.images.delete(imageId);
    this.target.removeImageSource?.(imageId);
  }

  removeVideoSource(streamId: string): void {
    const entry = this.videos.get(streamId);
    entry?.session.dispose();
    this.videos.delete(streamId);
    this.target.removeVideoSource?.(streamId);
  }

  async syncMediaSources(registrations: Iterable<MediaRegistration>): Promise<void> {
    const desiredImages = new Map<string, RegisteredImageSource>();
    const desiredVideos = new Map<string, RegisteredVideoSource>();

    for (const registration of registrations) {
      if (registration.kind === "image") {
        desiredImages.set(registration.id, registration);
      } else {
        desiredVideos.set(registration.id, registration);
      }
    }

    for (const imageId of this.images.keys()) {
      if (!desiredImages.has(imageId)) {
        this.removeImageSource(imageId);
      }
    }

    for (const streamId of this.videos.keys()) {
      if (!desiredVideos.has(streamId)) {
        this.removeVideoSource(streamId);
      }
    }

    for (const image of desiredImages.values()) {
      await this.registerImageSource(image.id, image.source);
    }

    for (const video of desiredVideos.values()) {
      const { id, source, ...options } = video;
      await this.registerVideoSource(id, source, options);
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
      syncedVersion: 0,
      version: (existing?.version ?? 0) + 1,
    });
    await this.loadImageSource(imageId);
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
    const version = (existing?.version ?? 0) + 1;
    this.videos.set(streamId, {
      registration,
      session: new VideoSession(streamId, registration, this.options),
      syncedFrames: new Set<number>(),
      syncedMetadata: null,
      syncedMetadataVersion: 0,
      version,
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
      );
    }
    if (entry.syncedVersion === entry.version) {
      return;
    }

    const version = entry.version;
    const frame = await entry.session.load();
    const current = this.images.get(imageId);
    if (!current || current.version !== version) {
      return;
    }

    this.target.setImage(imageId, frame.width, frame.height, frame.rgba);
    current.syncedVersion = version;
  }

  async loadVideoMetadata(streamId: string): Promise<VideoFrameMetadata> {
    const entry = this.requireVideoEntry(streamId);
    if (entry.syncedMetadataVersion === entry.version && entry.syncedMetadata) {
      return entry.syncedMetadata;
    }

    const version = entry.version;
    const metadata = await entry.session.loadMetadata();
    const current = this.videos.get(streamId);
    if (!current || current.version !== version) {
      return metadata;
    }

    this.target.setVideoMetadata(streamId, metadata.width, metadata.height, metadata.frameCount);
    current.syncedMetadata = metadata;
    current.syncedMetadataVersion = version;
    current.syncedFrames.clear();
    return metadata;
  }

  async loadVideoFrame(streamId: string, frame: number): Promise<void> {
    const entry = this.requireVideoEntry(streamId);
    if (entry.syncedFrames.has(frame) && entry.syncedMetadataVersion === entry.version) {
      return;
    }

    await this.syncDecodedFrames(streamId, entry, [frame]);
  }

  async loadFrameRequirements(requirements: FrameRequirementsPayload | string): Promise<void> {
    const parsed = parseFrameRequirements(requirements);

    await Promise.all(parsed.images.map((imageId) => this.loadImageSource(imageId)));
    await Promise.all(
      parsed.videos.map(async (video) => {
        const entry = this.requireVideoEntry(video.streamId);
        const frames = dedupeFrameNumbers(video.frames).filter(
          (frame) => !entry.syncedFrames.has(frame),
        );
        await this.syncDecodedFrames(video.streamId, entry, frames);
      }),
    );
  }

  private requireVideoEntry(streamId: string): VideoEntry {
    const entry = this.videos.get(streamId);
    if (!entry) {
      throw new LumenMediaError(
        "source_not_registered",
        `video source "${streamId}" is not registered`,
      );
    }
    return entry;
  }

  private resetVideoEntry(entry: VideoEntry): void {
    entry.session.clearFrames();
    entry.syncedFrames.clear();
    entry.syncedMetadataVersion = 0;
    entry.syncedMetadata = null;
  }

  private async syncDecodedFrames(
    streamId: string,
    entry: VideoEntry,
    frames: Iterable<number>,
  ): Promise<void> {
    const wantedFrames = dedupeFrameNumbers(frames).filter(
      (frame) => !entry.syncedFrames.has(frame),
    );
    if (wantedFrames.length === 0) {
      return;
    }

    const version = entry.version;
    await this.loadVideoMetadata(streamId);
    const decodedFrames = await entry.session.decodeFrames(wantedFrames);
    const current = this.videos.get(streamId);
    if (!current || current.version !== version) {
      for (const payload of decodedFrames.values()) {
        closeVideoFrame(payload.videoFrame);
      }
      return;
    }

    for (const [frame, payload] of decodedFrames) {
      await syncVideoFrameToTarget(this.target, streamId, frame, payload);
      const latest = this.videos.get(streamId);
      if (!latest || latest.version !== version) {
        closeVideoFrame(payload.videoFrame);
        continue;
      }
      latest.syncedFrames.add(frame);
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

function sameMediaInput(left: MediaSourceInput, right: MediaSourceInput): boolean {
  if (left === right) {
    return true;
  }

  if (typeof left === "string" && typeof right === "string") {
    return left === right;
  }

  if (isUrlObject(left) && isUrlObject(right)) {
    return left.href === right.href;
  }

  if (isRequestObject(left) && isRequestObject(right)) {
    return left.url === right.url && left.method === right.method;
  }

  if (isBufferSource(left) && isBufferSource(right)) {
    return left === right;
  }

  return false;
}

function sameVideoRegistration(
  left: VideoSourceRegistration,
  right: VideoSourceRegistration,
): boolean {
  return (
    sameMediaInput(left.source, right.source) &&
    left.fps === right.fps &&
    left.timelineMode === right.timelineMode &&
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

function isBufferSource(value: unknown): value is BufferSource {
  return value instanceof ArrayBuffer || ArrayBuffer.isView(value);
}

function isRequestObject(value: unknown): value is Request {
  return typeof Request !== "undefined" && value instanceof Request;
}

function isUrlObject(value: unknown): value is URL {
  return typeof URL !== "undefined" && value instanceof URL;
}

export type { MediaSourceDefaults };
