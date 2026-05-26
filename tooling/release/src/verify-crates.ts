import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { publishCrates } from "./crates.js";

const dirnamePath = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(dirnamePath, "../../..");

for (const crate of publishCrates) {
  await verifyCrateCompiles(crate);
}

async function verifyCrateCompiles(crate: string): Promise<void> {
  const result = await run("cargo", ["check", "-p", crate]);
  if (result.status !== "success") {
    throw new Error(`${crate} failed to compile:\n${result.output}`);
  }
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
