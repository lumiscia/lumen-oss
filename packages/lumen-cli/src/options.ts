import type { LumenComposition, LumenDiagnostic } from "@lumiscia/lumen-types";

/** A composition as JSON, or anything that turns into one with `toJSON()`, such as the SDK's `Composition`. */
export type CompositionSource = LumenComposition | { toJSON(): LumenComposition };

/** Frames `start` up to, not including, `end`. A missing side is the timeline's own start or end. */
export interface FrameRange {
  readonly start?: number;
  readonly end?: number;
}

export interface CommandOptions {
  /** Directory media sources must stay inside. Defaults to the current directory. */
  readonly mediaRoot?: string;
  /** Directory of fonts, which must exist. */
  readonly fontDir?: string;
  /** Hides what `lumen` prints, which is otherwise passed through to this process. */
  readonly quiet?: boolean;
  /** Stops `lumen` when aborted, and rejects with the signal's reason. */
  readonly signal?: AbortSignal;
}

export interface RenderOptions extends CommandOptions {
  /** `.mp4`, `.mov`, `.webm`, `.png`, `.jpg`, or a `%d` name for an image per frame. */
  readonly output: string;
  /** The frames to render. Defaults to the whole timeline. */
  readonly frames?: FrameRange;
  /** The frame an image output shows. Defaults to 0. */
  readonly frame?: number;
  /** Most glyph, stroke and decoration instances one text node may draw. */
  readonly maxTextGlyphs?: number;
  readonly codec?: "h264" | "hevc" | "av1" | "prores" | "vp9";
  /** A preset: `draft`, `standard`, `high`, `lossless`, or one the quality config defines. */
  readonly quality?: string;
  /** Path of a quality config JSON file. */
  readonly qualityConfig?: string;
  readonly bitDepth?: 8 | 10;
  /** Keeps transparency as an alpha channel in a `.mov` or `.webm`; `false` renders opaque. */
  readonly alpha?: boolean;
  /** Pins one encoder, with no fallback. */
  readonly encoder?: string;
  /** Bits per second, as `14000000`, `"14000k"` or `"14M"`. */
  readonly bitrate?: number | string;
  readonly crf?: number;
  readonly qp?: number;
  /** The encoder's own speed preset. */
  readonly preset?: string | number;
  /** 1 to 100, for a `.jpg` output. */
  readonly jpegQuality?: number;
}

export type CheckOptions = CommandOptions;

export interface PreviewOptions extends CommandOptions {
  /** `.png` or `.jpg`. */
  readonly output: string;
  /** How many frames the contact sheet shows. */
  readonly count?: number;
  readonly columns?: number;
  /** Width of the sheet in pixels. */
  readonly width?: number;
  /** The frames the sheet is taken from. Defaults to the whole timeline. */
  readonly frames?: FrameRange;
  readonly maxTextGlyphs?: number;
}

/** What `lumen check` found: `ok` is false when any diagnostic is an error. */
export interface CheckReport {
  readonly ok: boolean;
  readonly diagnostics: readonly LumenDiagnostic[];
}

/** What a finished `render()` or `preview()` wrote. */
export interface OutputResult {
  /** The output path, as given. */
  readonly output: string;
}
