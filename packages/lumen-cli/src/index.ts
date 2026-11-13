import { checkArgs, previewArgs, renderArgs } from "./args.js";
import { resolveBinary } from "./binary.js";
import type {
  CheckOptions,
  CheckReport,
  CompositionSource,
  OutputResult,
  PreviewOptions,
  RenderOptions,
} from "./options.js";
import { LumenCliError, run, type RunResult } from "./run.js";

export { LumenCliError } from "./run.js";
export type {
  CheckOptions,
  CheckReport,
  CommandOptions,
  CompositionSource,
  FrameRange,
  OutputResult,
  PreviewOptions,
  RenderOptions,
} from "./options.js";

function succeeded(result: RunResult): void {
  if (result.exitCode !== 0) {
    throw new LumenCliError(result.exitCode, result.signal, result.stderr);
  }
}

/** Renders `composition` with the `lumen` binary, like `lumen render - -o <output>`. */
export async function render(
  composition: CompositionSource,
  options: RenderOptions,
): Promise<OutputResult> {
  succeeded(await run(resolveBinary(), renderArgs(options), JSON.stringify(composition), options));
  return { output: options.output };
}

/** Writes a contact sheet of `composition`, like `lumen preview - -o <output>`. */
export async function preview(
  composition: CompositionSource,
  options: PreviewOptions,
): Promise<OutputResult> {
  succeeded(await run(resolveBinary(), previewArgs(options), JSON.stringify(composition), options));
  return { output: options.output };
}

/**
 * Checks `composition` without rendering it, like `lumen check -`. Resolves with the report
 * whether or not the composition has errors.
 */
export async function check(
  composition: CompositionSource,
  options: CheckOptions = {},
): Promise<CheckReport> {
  const result = await run(resolveBinary(), checkArgs(options), JSON.stringify(composition), {
    ...options,
    captureStdout: true,
  });
  if (result.exitCode === 0 || result.exitCode === 1) {
    const report = parseReport(result.stdout);
    if (report) {
      return report;
    }
  }
  succeeded(result);
  throw new Error(`lumen check printed something other than a report: ${result.stdout}`);
}

function parseReport(stdout: string): CheckReport | undefined {
  try {
    const report: unknown = JSON.parse(stdout);
    if (
      typeof report === "object" &&
      report !== null &&
      "ok" in report &&
      typeof report.ok === "boolean" &&
      "diagnostics" in report &&
      Array.isArray(report.diagnostics)
    ) {
      return { ok: report.ok, diagnostics: report.diagnostics };
    }
  } catch {
    // Not JSON, so not a report.
  }
  return undefined;
}
