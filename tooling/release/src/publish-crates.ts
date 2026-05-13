import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const dirnamePath = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(dirnamePath, "../../..");

const crates = [
  "lumen-engine-ffmpeg",
  "lumen-engine-gpu",
  "lumen-engine-macros",
  "lumen-engine-text",
  "lumen-engine",
  "lumen-server",
] as const;

const retryDelayMs = 20_000;
const maxAttempts = 6;

for (const crate of crates) {
  await publishCrate(crate);
}

async function publishCrate(crate: string): Promise<void> {
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    const result = await run("cargo", ["publish", "-p", crate]);
    if (result.status === "success") {
      return;
    }

    if (isAlreadyPublished(result.output)) {
      console.log(`${crate} is already published; continuing.`);
      return;
    }

    if (attempt < maxAttempts && shouldRetry(result.output)) {
      console.log(
        `${crate} publish failed while crates.io catches up; retrying in ${
          retryDelayMs / 1000
        }s (${attempt}/${maxAttempts})...`,
      );
      await sleep(retryDelayMs);
      continue;
    }

    throw new Error(`${crate} publish failed:\n${result.output}`);
  }
}

function shouldRetry(output: string): boolean {
  return output.includes("no matching package named") || output.includes("failed to select a version");
}

function isAlreadyPublished(output: string): boolean {
  return output.includes("already uploaded") || output.includes("already exists");
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

function run(
  command: string,
  args: readonly string[],
): Promise<{ status: "success" } | { status: "failed"; output: string }> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: repoRoot,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";

    child.stdout.on("data", (chunk: Buffer) => {
      process.stdout.write(chunk);
      output += chunk.toString("utf8");
    });
    child.stderr.on("data", (chunk: Buffer) => {
      process.stderr.write(chunk);
      output += chunk.toString("utf8");
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      resolvePromise(code === 0 ? { status: "success" } : { status: "failed", output });
    });
  });
}
