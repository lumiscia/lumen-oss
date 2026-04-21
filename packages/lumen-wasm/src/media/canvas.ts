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

  throw new Error("A canvas implementation is required to bridge media into lumen-wasm");
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
  const rgba = new Uint8Array(payload.width * payload.height * 4);
  await payload.videoFrame.copyTo(rgba, {
    format: "RGBA",
    layout: [{ offset: 0, stride: payload.width * 4 }],
  });

  return {
    width: payload.width,
    height: payload.height,
    rgba,
    videoFrame: payload.videoFrame,
  };
}

export function createVideoFramePayload(
  videoFrame: VideoFrame,
  width: number,
  height: number,
): VideoFramePayload {
  return {
    width,
    height,
    videoFrame,
  };
}

export async function syncVideoFrameToTarget(
  target: NativeVideoFrameTarget,
  streamId: string,
  frame: number,
  payload: VideoFramePayload,
): Promise<void> {
  try {
    if (typeof target.setVideoFrameObject === "function") {
      await target.setVideoFrameObject(
        streamId,
        frame,
        payload.videoFrame,
        payload.width,
        payload.height,
      );
      return;
    }

    const pixels = await decodeVideoFramePixels(payload);
    target.setVideoFrame(streamId, frame, pixels.width, pixels.height, pixels.rgba);
  } finally {
    closeVideoFrame(payload.videoFrame);
  }
}

export function closeVideoFrame(frame: VideoFrame | undefined): void {
  frame?.close();
}
