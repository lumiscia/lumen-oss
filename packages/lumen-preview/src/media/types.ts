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

export interface RegisteredImageSource {
  kind: "image";
  id: string;
  source: MediaSourceInput;
}

export interface RegisteredVideoSource extends Omit<VideoSourceRegistration, "source"> {
  kind: "video";
  id: string;
  source: MediaSourceInput;
}

export type MediaRegistration = RegisteredImageSource | RegisteredVideoSource;

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
  clearVideos(): void;
  clearVideoSource(streamId: string): void;
  removeFontFamily?(fontFamily: string): void;
  removeImageSource?(imageId: string): void;
  removeVideoSource?(streamId: string): void;
  hasFont?(fontFamily: string): boolean;
  hasImage(imageId: string): boolean;
  hasVideoFrame(streamId: string, frame: number): boolean;
  setFont?(fontFamily: string, bytes: Uint8Array): void;
  setImage(imageId: string, width: number, height: number, rgba: Uint8Array): void;
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
