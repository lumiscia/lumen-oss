import { createHash } from "node:crypto";
import { readFile, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";

const dirnamePath = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(dirnamePath, "../../..");
const wasmCrate = "lumen-wasm";
const wasmName = "lumen_wasm";
const wasmTarget = "wasm32-unknown-unknown";
const initialMemory = "134217728";
const maxMemory = "2147483648";

const targets = [
  { bindgenTarget: "bundler", out: "bundler" },
  { bindgenTarget: "web", out: "browser" },
  { bindgenTarget: "nodejs", out: "node" },
  { bindgenTarget: "no-modules", out: "no-modules" },
] as const;

const options = parseArgs();
const mode = options.mode;
const bindingsDir = options.outDir;
const profile = mode === "release" ? "release" : "debug";
const wasmPath = join(repoRoot, "target", wasmTarget, profile, `${wasmName}.wasm`);

await run("cargo", [
  "build",
  ...(mode === "release" ? ["--release"] : []),
  "--package",
  wasmCrate,
  "--target",
  wasmTarget,
]);

await Promise.all(
  targets.map((target) => rm(join(bindingsDir, target.out), { recursive: true, force: true })),
);

for (const target of targets) {
  await run("wasm-bindgen", [
    ...bindgenArgs(mode),
    "--target",
    target.bindgenTarget,
    "--out-dir",
    join(bindingsDir, target.out),
    "--out-name",
    wasmName,
    wasmPath,
  ]);
}

if (mode === "release") {
  console.log("Running wasm-opt for generated WASM targets...");
  await Promise.all(
    targets.map(async (target) => {
      const wasm = join(bindingsDir, target.out, `${wasmName}_bg.wasm`);
      await run("pnpm", ["exec", "wasm-opt", "-Oz", "-o", wasm, wasm]);
    }),
  );
}

await verifySharedWasm();

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

function bindgenArgs(nextMode: "debug" | "release") {
  return nextMode === "debug" ? ["--debug", "--keep-debug"] : [];
}

async function verifySharedWasm() {
  const hashes = new Map<string, string>();
  for (const target of targets) {
    const wasm = join(bindingsDir, target.out, `${wasmName}_bg.wasm`);
    const digest = await hashFile(wasm);
    hashes.set(target.out, digest);
  }

  const uniqueHashes = new Set(hashes.values());
  if (uniqueHashes.size === 1) {
    console.log(`Generated ${mode} bindings. Shared WASM SHA-256: ${[...uniqueHashes][0]}`);
    return;
  }

  const lines = [...hashes].map(([target, digest]) => `- ${target}: ${digest}`).join("\n");
  throw new Error(`Generated target wasm files differ:\n${lines}`);
}

async function hashFile(path: string) {
  const bytes = await readFile(path);
  return createHash("sha256").update(bytes).digest("hex");
}

function run(command: string, args: string[]) {
  return new Promise<void>((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: repoRoot,
      env: {
        ...process.env,
        RUSTFLAGS: `-C link-arg=--initial-memory=${initialMemory} -C link-arg=--max-memory=${maxMemory}`,
      },
      stdio: "inherit",
    });

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
