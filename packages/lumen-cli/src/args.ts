import type { flags } from "./flags.js";
import type { CheckOptions, FrameRange, PreviewOptions, RenderOptions } from "./options.js";

type Subcommand = keyof typeof flags;

type Flags<Command extends Subcommand> = readonly (readonly [
  flag: `--${(typeof flags)[Command][number]}`,
  value: string | number | boolean | undefined,
])[];

/**
 * `--flag=value` for each flag of `Command` with a value, so an unset option adds nothing. One
 * argument each, so a value that starts with `-` is not read as a flag.
 */
function flagArgs<Command extends Subcommand>(pairs: Flags<Command>): string[] {
  return pairs.flatMap(([flag, value]) => (value === undefined ? [] : [`${flag}=${value}`]));
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
    ...flagArgs<"render">([
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
      ["--alpha", options.alpha],
    ]),
  ];
}

export function checkArgs(options: CheckOptions): string[] {
  return [
    "check",
    "-",
    ...flagArgs<"check">([
      ["--format", "json"],
      ["--media-root", options.mediaRoot],
      ["--font-dir", options.fontDir],
    ]),
  ];
}

export function previewArgs(options: PreviewOptions): string[] {
  return [
    "preview",
    "-",
    ...flagArgs<"preview">([
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
