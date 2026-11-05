import { mkdir, rename, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";

const dirnamePath = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(dirnamePath, "../../..");
const wasmCrate = "lumen-wasm";
const wasmName = "lumen_wasm";
const wasmTarget = "wasm32-unknown-unknown";

const { mode, outDir } = parseArgs();
const profile = mode === "release" ? "release" : "debug";
const targetDir = resolve(repoRoot, process.env.CARGO_TARGET_DIR || "target");
const wasmPath = join(targetDir, wasmTarget, profile, `${wasmName}.wasm`);

await run("cargo", [
  "build",
  ...(mode === "release" ? ["--release"] : []),
  "--package",
  wasmCrate,
  "--target",
  wasmTarget,
]);

// Everything is written beside the output and swapped in last, so a failed build leaves it intact.
const tmpDir = `${outDir}.tmp-${process.pid}`;
await mkdir(tmpDir);
try {
  await run("wasm-bindgen", [
    ...(mode === "debug" ? ["--debug", "--keep-debug"] : []),
    "--target",
    "web",
    "--out-dir",
    tmpDir,
    "--out-name",
    wasmName,
    wasmPath,
  ]);

  if (mode === "release") {
    const wasm = join(tmpDir, `${wasmName}_bg.wasm`);
    await run("pnpm", ["exec", "wasm-opt", "-Oz", "-o", wasm, wasm]);
  }

  await rm(outDir, { recursive: true, force: true });
  await rename(tmpDir, outDir);
} finally {
  await rm(tmpDir, { recursive: true, force: true });
}
console.log(
  `Generated ${mode} bindings in ${outDir}. Run \`pnpm --filter @lumiscia/lumen-bindings build\` to bundle them into the preview worker.`,
);

function parseArgs() {
  let mode: "debug" | "release" | undefined;
  let outDir: string | undefined;
  const args = process.argv.slice(2);

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--") {
      continue;
    }
    if (arg === "--out-dir") {
      outDir = resolve(repoRoot, requiredValue(args, index));
      index += 1;
      continue;
    }
    if (arg === "--mode") {
      mode = readMode(requiredValue(args, index));
      index += 1;
      continue;
    }
    if (arg === "debug" || arg === "release") {
      mode = arg;
      continue;
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  if (!outDir) {
    throw new Error("Missing required --out-dir <path>.");
  }

  return { mode: mode ?? "release", outDir };
}

function readMode(requested: string) {
  if (requested === "debug" || requested === "release") {
    return requested;
  }

  throw new Error(`Unknown binding mode "${requested}". Expected "debug" or "release".`);
}

function requiredValue(args: string[], index: number) {
  const value = args[index + 1];
  if (!value) {
    throw new Error(`Missing value for ${args[index]}.`);
  }
  return value;
}

function run(command: string, args: string[]) {
  return new Promise<void>((resolvePromise, reject) => {
    const child = spawn(command, args, { cwd: repoRoot, stdio: "inherit" });

    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (code === 0) {
        resolvePromise();
        return;
      }
      reject(
        new Error(`${command} ${args.join(" ")} failed with ${signal ?? `exit code ${code}`}`),
      );
    });
  });
}
