import type { BlobSourceOptions, InputFormat, UrlSourceOptions } from "mediabunny";

export interface FrameRequirementsPayload {
  images: string[];
  videos: FrameRequirementsVideoPayload[];
}

export interface FrameRequirementsVideoPayload {
  streamId: string;
  frames: number[];
}

export type MediaSourceInput =
  | string
  | URL
  | Request
  | Blob
  | BufferSource
  | BlobMediaSourceInput
  | BufferMediaSourceInput
  | UrlMediaSourceInput;

export interface BlobMediaSourceInput {
  kind: "blob";
  blob: Blob;
  options?: BlobSourceOptions;
}

export interface BufferMediaSourceInput {
  kind: "buffer";
  bytes: BufferSource;
}

export interface UrlMediaSourceInput {
  kind: "url";
  url: string | URL | Request;
  options?: UrlSourceOptions;
}

export interface VideoSourceRegistration {
  source: MediaSourceInput;
  /** The rate source frames are numbered at. Defaults to the stream's own rate. */
  fps?: number | null;
  formats?: InputFormat[];
  track?: number | "primary";
}

export interface ImageSourceRegistration {
  imageId: string;
  source: MediaSourceInput;
}

export interface DecodedImageFrame {
  width: number;
  height: number;
  rgba: Uint8Array;
}

export interface VideoFramePayload {
  width: number;
  height: number;
  videoFrame: VideoFrame;
  /** The colour space the source is tagged with, which can leave out the matrix and range. */
  sourceColorSpace?: VideoColorSpaceInit;
  /** Degrees clockwise the frame's native planes turn to show upright. */
  rotation?: 0 | 90 | 180 | 270;
}

export interface VideoFramePixels extends DecodedImageFrame {
  videoFrame?: VideoFrame;
}

export interface VideoFrameMetadata {
  width: number;
  height: number;
  duration: number;
  fps: number;
  frameCount: number;
  firstTimestamp: number;
  mimeType: string | null;
  trackId: number;
  trackNumber: number;
  codec: string | null;
}

export interface LumenMediaTarget {
  clear(): void;
  removeEnvironmentSource?(sourceId: string): void;
  removeFontFamily?(fontFamily: string): void;
  removeImageSource?(imageId: string): void;
  removeLutSource?(sourceId: string): void;
  removeModelSource?(modelId: string): void;
  removeVideoSource?(streamId: string): void;
  hasEnvironment?(sourceId: string): boolean;
  hasImage(imageId: string): boolean;
  hasLut?(sourceId: string): boolean;
  hasModel?(modelId: string): boolean;
  hasVideoFrame(streamId: string, frame: number): boolean;
  /** Takes an `.hdr` or `.exr` file's bytes, which the engine reads when a scene first lights with it. */
  setEnvironment?(sourceId: string, bytes: Uint8Array): void;
  setFont?(fontFamily: string, bytes: Uint8Array): void;
  setImage(imageId: string, width: number, height: number, rgba: Uint8Array): void;
  /** Takes a `.cube` file's bytes, which the engine reads when a `color_grade` first grades with it. */
  setLut?(sourceId: string, bytes: Uint8Array): void;
  /** Takes a `.glb` file's bytes, which the engine reads when a render first draws the model. */
  setModel?(modelId: string, bytes: Uint8Array): void;
  setVideoFrameCacheBytes?(bytes: number): void;
  setVideoMetadata(
    streamId: string,
    width: number,
    height: number,
    frameCount: number,
    fps: number,
  ): void;
  setVideoFrame(
    streamId: string,
    frame: number,
    width: number,
    height: number,
    rgba: Uint8Array,
  ): void;
}

export interface NativeVideoFrameTarget extends LumenMediaTarget {
  /**
   * Takes a frame's native YUV planes, as `VideoFrame.copyTo` lays them out, for the engine to
   * convert. `width` and `height` are the copied size before `rotation`; `displayWidth` and
   * `displayHeight` are the size it shows at, after rotation and pixel aspect ratio.
   */
  setVideoFrameYuv?(
    streamId: string,
    frame: number,
    width: number,
    height: number,
    format: string,
    data: Uint8Array,
    offsets: Uint32Array,
    strides: Uint32Array,
    matrix: string | undefined,
    fullRange: boolean,
    rotation: number,
    displayWidth: number,
    displayHeight: number,
    primaries?: string,
    transfer?: string,
  ): void;
  setVideoFrameObject?(
    streamId: string,
    frame: number,
    videoFrame: VideoFrame,
    width: number,
    height: number,
  ): void | Promise<void>;
}

export interface LumenMediaBridgeOptions {
  blobCacheBytes?: number;
  /** Byte budget shared by all decoded video frames cached in the wasm media store. */
  frameCacheCapacity?: number;
  formats?: InputFormat[];
  urlCacheBytes?: number;
}
