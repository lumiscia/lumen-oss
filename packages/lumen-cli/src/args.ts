import type { CheckOptions, FrameRange, PreviewOptions, RenderOptions } from "./options.js";

type Flags = readonly (readonly [flag: string, value: string | number | boolean | undefined])[];

/** `--flag value` for each flag with a value, so an unset option adds nothing. */
function flagArgs(flags: Flags): string[] {
  return flags.flatMap(([flag, value]) => (value === undefined ? [] : [flag, String(value)]));
}

function framesArg(frames: FrameRange | undefined): string | undefined {
  if (frames === undefined || (frames.start === undefined && frames.end === undefined)) {
    return undefined;
  }
  return `${frames.start ?? ""}..${frames.end ?? ""}`;
}

export function renderArgs(options: RenderOptions): string[] {
  return [
    "render",
    "-",
    ...flagArgs([
      ["--output", options.output],
      ["--frames", framesArg(options.frames)],
      ["--frame", options.frame],
      ["--media-root", options.mediaRoot],
      ["--font-dir", options.fontDir],
      ["--max-text-glyphs", options.maxTextGlyphs],
      ["--codec", options.codec],
      ["--quality", options.quality],
      ["--quality-config", options.qualityConfig],
      ["--bit-depth", options.bitDepth],
      ["--encoder", options.encoder],
      ["--bitrate", options.bitrate],
      ["--crf", options.crf],
      ["--qp", options.qp],
      ["--preset", options.preset],
      ["--jpeg-quality", options.jpegQuality],
    ]),
    ...(options.alpha === undefined ? [] : [`--alpha=${options.alpha}`]),
  ];
}

export function checkArgs(options: CheckOptions): string[] {
  return [
    "check",
    "-",
    "--format",
    "json",
    ...flagArgs([
      ["--media-root", options.mediaRoot],
      ["--font-dir", options.fontDir],
    ]),
  ];
}

export function previewArgs(options: PreviewOptions): string[] {
  return [
    "preview",
    "-",
    ...flagArgs([
      ["--output", options.output],
      ["--count", options.count],
      ["--columns", options.columns],
      ["--width", options.width],
      ["--frames", framesArg(options.frames)],
      ["--media-root", options.mediaRoot],
      ["--font-dir", options.fontDir],
      ["--max-text-glyphs", options.maxTextGlyphs],
    ]),
  ];
}
