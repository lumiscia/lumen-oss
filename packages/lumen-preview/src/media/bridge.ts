import { ALL_FORMATS, type InputFormat } from "mediabunny";

import { closeVideoFrame, syncVideoFrameToTarget, takesYuvPlanes } from "./canvas.js";
import { LumenMediaError } from "./errors.js";
import { ImageSession } from "./image-session.js";
import { readSceneFile } from "./model.js";
import {
  DEFAULT_BLOB_CACHE_BYTES,
  DEFAULT_URL_CACHE_BYTES,
  sameMediaInput,
  type MediaSourceDefaults,
} from "./source.js";
import type {
  FileMediaKind,
  FrameRequirementsPayload,
  LumenMediaBridgeOptions,
  MediaSourceInput,
  NativeVideoFrameTarget,
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
// Removing an entry aborts its reads.
type ImageEntry = {
  abort: AbortController;
  session: ImageSession;
  source: MediaSourceInput;
};

type FileEntry = {
  abort: AbortController;
  source: MediaSourceInput;
  /** The read and hand-over in flight, which loads of the same entry join. */
  upload: Promise<void> | null;
};

type VideoEntry = {
  registration: VideoSourceRegistration;
  session: VideoSession;
  syncedMetadata: VideoFrameMetadata | null;
};

/** Hands each kind of media to the wasm target: images and video frames decoded, files as bytes. */
export class LumenMediaBridge<TTarget extends NativeVideoFrameTarget> {
  private readonly files: Record<FileMediaKind, Map<string, FileEntry>> = {
    environment: new Map(),
    lut: new Map(),
    model: new Map(),
  };
  private readonly images = new Map<string, ImageEntry>();
  private readonly options: NormalizedBridgeOptions;
  private readonly videos = new Map<string, VideoEntry>();

  constructor(
    private readonly target: TTarget,
    options: LumenMediaBridgeOptions = {},
  ) {
    this.options = normalizeBridgeOptions(options);
    target.setVideoFrameCacheBytes(this.options.frameCacheCapacity);
  }

  clear(): void {
    const files = Object.values(this.files);
    for (const entry of [...this.images.values(), ...files.flatMap((map) => [...map.values()])]) {
      entry.abort.abort();
    }
    this.images.clear();
    for (const map of files) {
      map.clear();
    }
    for (const video of this.videos.values()) {
      video.session.dispose();
    }
    this.videos.clear();
  }

  removeImageSource(imageId: string): void {
    this.images.get(imageId)?.abort.abort();
    this.images.delete(imageId);
    this.target.removeImageSource(imageId);
  }

  removeFile(kind: FileMediaKind, source: string): void {
    this.files[kind].get(source)?.abort.abort();
    this.files[kind].delete(source);
    this.target.removeFile(kind, source);
  }

  removeVideoSource(streamId: string): void {
    this.videos.get(streamId)?.session.dispose();
    this.videos.delete(streamId);
    this.target.removeVideoSource(streamId);
  }

  async registerImageSource(imageId: string, source: MediaSourceInput): Promise<void> {
    const existing = this.images.get(imageId);
    if (existing && sameMediaInput(existing.source, source)) {
      await this.loadImageSource(imageId);
      return;
    }

    this.removeImageSource(imageId);
    const abort = new AbortController();
    this.images.set(imageId, {
      abort,
      session: new ImageSession(imageId, source, abort.signal),
      source,
    });
    await this.loadImageSource(imageId);
  }

  async registerVideoSource(
    streamId: string,
    source: MediaSourceInput,
    options: Omit<VideoSourceRegistration, "source"> = {},
  ): Promise<VideoFrameMetadata> {
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

  /**
   * Hands the file `source` holds to the nodes that name `id`: a binary glTF (`.glb`) model to
   * `model_3d`, an `.hdr` or `.exr` file to `environment_map` params, a `.cube` file to
   * `color_grade`'s `lut_source`.
   */
  async registerFile(kind: FileMediaKind, id: string, source: MediaSourceInput): Promise<void> {
    const files = this.files[kind];
    let entry = files.get(id);
    if (!entry || !sameMediaInput(entry.source, source)) {
      this.removeFile(kind, id);
      entry = { abort: new AbortController(), source, upload: null };
      files.set(id, entry);
    }
    if (this.target.hasFile(kind, id)) {
      return;
    }

    const current = entry;
    current.upload ??= readSceneFile(kind, id, source, current.abort.signal)
      .then((bytes) => {
        if (files.get(id) === current) {
          this.target.setFile(kind, id, bytes);
        }
      })
      .finally(() => {
        current.upload = null;
      });
    await current.upload;
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
