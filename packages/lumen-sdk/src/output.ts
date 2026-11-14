import { outputFiles, type RenderOutput } from "@lumiscia/lumen-types";

export type { FrameRange, OutputFormat, RenderOutput } from "@lumiscia/lumen-types";

/** The content type and file extension of what a render of `output` downloads as. */
export function outputFile(output: RenderOutput = {}): {
  readonly contentType: string;
  readonly extension: string;
} {
  return outputFiles[output.format ?? "mp4"];
}
