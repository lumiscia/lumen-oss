import { spawn } from "node:child_process";
import process from "node:process";

import { forwardSignals } from "./signals.js";

const STDERR_TAIL_CHARS = 16_384;

/** `lumen` exited with a failure: a non-zero code, or a signal. */
export class LumenCliError extends Error {
  /** The exit code, or `null` when a signal ended the process. */
  readonly exitCode: number | null;
  /** The signal that ended the process, such as `"SIGTERM"`, or `null` when it exited. */
  readonly signal: string | null;
  /** The end of what `lumen` wrote to stderr. */
  readonly stderr: string;

  constructor(exitCode: number | null, signal: string | null, stderr: string) {
    const how = signal === null ? `exited with code ${exitCode}` : `was stopped by ${signal}`;
    super(`lumen ${how}${stderr.trim() === "" ? "" : `:\n${stderr.trim()}`}`);
    this.name = "LumenCliError";
    this.exitCode = exitCode;
    this.signal = signal;
    this.stderr = stderr;
  }
}

export interface RunOptions {
  /** Keeps stdout for the caller instead of passing it through. */
  readonly captureStdout?: boolean | undefined;
  readonly quiet?: boolean | undefined;
  readonly signal?: AbortSignal | undefined;
}

export interface RunResult {
  readonly exitCode: number | null;
  readonly signal: string | null;
  readonly stdout: string;
  readonly stderr: string;
}

/**
 * Runs `binary` with `input` as its stdin and resolves when it has exited, whatever the status.
 * Rejects when it can't start, or when `options.signal` aborts.
 */
export function run(
  binary: string,
  args: readonly string[],
  input: string,
  options: RunOptions,
): Promise<RunResult> {
  const { captureStdout = false, quiet = false, signal } = options;
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }

    const child = spawn(binary, args, {
      stdio: ["pipe", captureStdout ? "pipe" : quiet ? "ignore" : "inherit", "pipe"],
    });
    let stdout = "";
    let stderr = "";

    child.stdout?.setEncoding("utf8").on("data", (chunk: string) => {
      stdout += chunk;
    });
    const childStderr = child.stderr?.setEncoding("utf8");
    let waitingForDrain = false;
    const resumeStderr = () => {
      waitingForDrain = false;
      childStderr?.resume();
    };
    childStderr?.on("data", (chunk: string) => {
      stderr = (stderr + chunk).slice(-STDERR_TAIL_CHARS);
      // The cut can fall inside a surrogate pair.
      if (/^[\uDC00-\uDFFF]/.test(stderr)) {
        stderr = stderr.slice(1);
      }
      // Waits for a slow reader of this process's stderr instead of queueing the output for it.
      if (!quiet && !process.stderr.write(chunk) && !waitingForDrain) {
        waitingForDrain = true;
        childStderr.pause();
        process.stderr.once("drain", resumeStderr);
      }
    });

    // The binary can exit before reading all of its input; its exit status says what happened.
    child.stdin?.on("error", () => {});
    child.stdin?.end(input);

    const abort = () => {
      child.kill();
    };
    signal?.addEventListener("abort", abort, { once: true });

    const stopForwarding = forwardSignals(child);
    const finished = () => {
      signal?.removeEventListener("abort", abort);
      process.stderr.off("drain", resumeStderr);
      stopForwarding();
    };

    child.on("error", (error) => {
      finished();
      reject(new Error(`could not run ${binary}: ${error.message}`, { cause: error }));
    });
    child.on("close", (exitCode, exitSignal) => {
      finished();
      if (signal?.aborted) {
        reject(signal.reason);
      } else {
        resolve({ exitCode, signal: exitSignal, stdout, stderr });
      }
    });
  });
}
