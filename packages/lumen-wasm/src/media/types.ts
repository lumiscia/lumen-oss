import type { BlobSourceOptions, InputFormat, Source, UrlSourceOptions } from "mediabunny";

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
  | Source
  | BlobMediaSourceInput
  | BufferMediaSourceInput
  | SourceMediaSourceInput
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

export interface SourceMediaSourceInput {
  kind: "source";
  source: Source;
}

export interface UrlMediaSourceInput {
  kind: "url";
  url: string | URL | Request;
  options?: UrlSourceOptions;
}

export type VideoTimelineMode = "fixed-fps" | "native-samples";

export interface VideoSourceRegistration {
  source: MediaSourceInput;
  fps?: number | null;
  formats?: InputFormat[];
  timelineMode?: VideoTimelineMode;
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
  timelineMode: VideoTimelineMode;
}

export interface LumenMediaTarget {
  clear(): void;
  clearVideos(): void;
  clearVideoSource(streamId: string): void;
  removeImageSource?(imageId: string): void;
  removeVideoSource?(streamId: string): void;
  hasImage(imageId: string): boolean;
  hasVideoFrame(streamId: string, frame: number): boolean;
  setImage(imageId: string, width: number, height: number, rgba: Uint8Array): void;
  setVideoMetadata(streamId: string, width: number, height: number, frameCount: number): void;
  setVideoFrame(
    streamId: string,
    frame: number,
    width: number,
    height: number,
    rgba: Uint8Array,
  ): void;
}

export interface NativeVideoFrameTarget extends LumenMediaTarget {
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
  frameCacheCapacity?: number;
  formats?: InputFormat[];
  urlCacheBytes?: number;
}
