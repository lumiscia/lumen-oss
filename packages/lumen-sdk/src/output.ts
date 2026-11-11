/** What a render writes. A sequence is one ZIP of its frames. */
export type OutputFormat = "mp4" | "png" | "jpeg" | "png_sequence" | "jpeg_sequence";

/**
 * The frames of the timeline to render, as `start..end` with `end` not included, like the
 * preview player's `range: { in, out }`. `start` defaults to 0 and `end` to the end of the
 * timeline, except for a still, whose `end` is the frame after `start`.
 */
export interface FrameRange {
  readonly start?: number;
  readonly end?: number;
}

/**
 * What to render: the format and which frames. Without it a render is an MP4 of the whole
 * timeline. `quality`, `codec` and `bitDepth` on `RenderOptions` apply to `mp4` only.
 */
export interface RenderOutput {
  /** Defaults to `mp4`. */
  readonly format?: OutputFormat;
  readonly frames?: FrameRange;
  /** From 1 to 100, for `jpeg` and `jpeg_sequence` only. Defaults to 90. */
  readonly jpegQuality?: number;
}

/** The content type and file extension of what a render of `output` downloads as. */
export function outputFile(output: RenderOutput = {}): {
  readonly contentType: string;
  readonly extension: string;
} {
  switch (output.format ?? "mp4") {
    case "mp4":
      return { contentType: "video/mp4", extension: "mp4" };
    case "png":
      return { contentType: "image/png", extension: "png" };
    case "jpeg":
      return { contentType: "image/jpeg", extension: "jpg" };
    case "png_sequence":
    case "jpeg_sequence":
      return { contentType: "application/zip", extension: "zip" };
  }
}

/** The request body's `output`, whose JPEG quality is `jpeg_quality` on the wire. */
export function outputBody(output: RenderOutput): Record<string, unknown> {
  const { jpegQuality, ...rest } = output;
  return { ...rest, ...(jpegQuality !== undefined ? { jpeg_quality: jpegQuality } : {}) };
}
