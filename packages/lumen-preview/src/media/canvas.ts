import type { VideoSample } from "mediabunny";

import type {
  DecodedImageFrame,
  NativeVideoFrameTarget,
  VideoFramePayload,
  VideoFramePixels,
} from "./types.js";

export function createCanvas(width: number, height: number) {
  if (typeof document !== "undefined") {
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) {
      throw new Error("2D canvas context is unavailable");
    }

    return { canvas, context };
  }

  if (typeof OffscreenCanvas !== "undefined") {
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext("2d");
    if (!context) {
      throw new Error("2D offscreen canvas context is unavailable");
    }

    return { canvas, context };
  }

  throw new Error("A canvas implementation is required to bridge media into lumen-preview");
}

export async function decodeImageBitmapToRgba(bitmap: ImageBitmap): Promise<DecodedImageFrame> {
  try {
    const { context } = createCanvas(bitmap.width, bitmap.height);
    context.drawImage(bitmap, 0, 0);
    const rgba = new Uint8Array(context.getImageData(0, 0, bitmap.width, bitmap.height).data);
    return { width: bitmap.width, height: bitmap.height, rgba };
  } finally {
    bitmap.close?.();
  }
}

export async function decodeVideoFramePixels(
  payload: VideoFramePayload,
): Promise<VideoFramePixels> {
  const { videoFrame, width, height } = payload;
  const rgba = new Uint8Array(width * height * 4);
  const layout = [{ offset: 0, stride: width * 4 }];
  if (videoFrame.format && RGB_FORMATS.has(videoFrame.format)) {
    // Safari ignores `format` and copies the native layout, so RGB frames are copied as they are.
    await videoFrame.copyTo(rgba, { layout });
    if (videoFrame.format.startsWith("BGR")) {
      for (let i = 0; i < rgba.length; i += 4) {
        const blue = rgba[i]!;
        rgba[i] = rgba[i + 2]!;
        rgba[i + 2] = blue;
      }
    }
    if (videoFrame.format.endsWith("X")) {
      for (let i = 3; i < rgba.length; i += 4) {
        rgba[i] = 255;
      }
    }
  } else {
    await videoFrame.copyTo(rgba, { format: "RGBA", layout });
  }

  return { width, height, rgba, videoFrame };
}

export function createVideoFramePayload(
  videoFrame: VideoFrame,
  width: number,
  height: number,
  sourceColorSpace?: VideoColorSpaceInit,
): VideoFramePayload {
  return { width, height, videoFrame, ...(sourceColorSpace && { sourceColorSpace }) };
}

/** Bakes a sample's rotation into a new frame, because `VideoFrame.copyTo` returns coded pixels. */
export function createUprightVideoFrame(
  sample: VideoSample,
  width: number,
  height: number,
): VideoFrame {
  const { canvas, context } = createCanvas(width, height);
  sample.draw(context, 0, 0, width, height);
  return new VideoFrame(canvas, { timestamp: sample.microsecondTimestamp });
}

export async function syncVideoFrameToTarget(
  target: NativeVideoFrameTarget,
  streamId: string,
  frame: number,
  payload: VideoFramePayload,
  isCurrent: () => boolean = () => true,
): Promise<void> {
  try {
    if (typeof target.setVideoFrameObject === "function") {
      if (isCurrent()) {
        await target.setVideoFrameObject(
          streamId,
          frame,
          payload.videoFrame,
          payload.width,
          payload.height,
        );
      }
      return;
    }

    if (target.setVideoFrameYuv && canCopyYuv(payload)) {
      await copyYuvToTarget(target, streamId, frame, payload, isCurrent);
      return;
    }

    const pixels = await decodeVideoFramePixels(payload);
    if (isCurrent()) {
      target.setVideoFrame(streamId, frame, pixels.width, pixels.height, pixels.rgba);
    }
  } finally {
    closeVideoFrame(payload.videoFrame);
  }
}

const RGB_FORMATS = new Set<string>(["RGBA", "RGBX", "BGRA", "BGRX"]);
const YUV_FORMATS = new Set<string>(["NV12", "I420", "I420A"]);

function canCopyYuv({ videoFrame, width, height }: VideoFramePayload): boolean {
  const rect = videoFrame.visibleRect;
  return (
    !!videoFrame.format &&
    YUV_FORMATS.has(videoFrame.format) &&
    rect?.width === width &&
    rect.height === height
  );
}

/**
 * Hands the engine the frame's YUV, so it converts with the same matrix as a render rather than the
 * browser's. The matrix comes from the source's tags, because browsers report their own guess on
 * the frame. The range comes from the frame, because Safari expands samples to full range.
 */
async function copyYuvToTarget(
  target: NativeVideoFrameTarget,
  streamId: string,
  frame: number,
  { videoFrame, width, height, sourceColorSpace }: VideoFramePayload,
  isCurrent: () => boolean,
): Promise<void> {
  const data = new Uint8Array(videoFrame.allocationSize());
  const layout = await videoFrame.copyTo(data);
  if (!isCurrent()) {
    return;
  }
  target.setVideoFrameYuv!(
    streamId,
    frame,
    width,
    height,
    videoFrame.format!,
    data,
    Uint32Array.from(layout, (plane) => plane.offset),
    Uint32Array.from(layout, (plane) => plane.stride),
    sourceColorSpace?.matrix ?? undefined,
    videoFrame.colorSpace.fullRange ?? sourceColorSpace?.fullRange ?? false,
  );
}

export function closeVideoFrame(frame: VideoFrame | undefined): void {
  frame?.close();
}
