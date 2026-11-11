import { execFileSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const enginePath = "crates/engine/src/json/mod.rs";
const constantPattern = /(pub const SCHEMA_VERSION: &str = ")(\d+\.\d+)(";)/;
const textExtensions = /\.(json|md|ts|tsx|svelte|rs|toml|html|js|mjs)$/;
// Generated output follows from the engine constant, and a changelog or changeset records the
// versions of its time.
const skipped = /(^|\/)(generated|CHANGELOG\.md$|\.changeset\/|node_modules\/|pnpm-lock\.yaml$)/;

const next = process.argv[2];
if (next === undefined || !/^\d+\.\d+$/.test(next)) {
  throw new Error(`Expected a Lumen major.minor such as 0.6, got ${next}`);
}

const engine = await readFile(resolve(repoRoot, enginePath), "utf8");
const current = constantPattern.exec(engine)?.[2];
if (current === undefined) {
  throw new Error(`Could not find SCHEMA_VERSION in ${enginePath}`);
}

if (current === next) {
  console.log(`Lumen schema version is already ${next}`);
} else {
  const declaration = new RegExp(
    String.raw`((["']?)lumenSchemaVersion\2\s*:\s*)(["'])${current.replace(".", String.raw`\.`)}\3`,
    "g",
  );
  const tracked = execFileSync("git", ["ls-files", "-z"], { cwd: repoRoot, encoding: "utf8" })
    .split("\0")
    .filter((path) => textExtensions.test(path) && !skipped.test(path));

  for (const path of tracked) {
    const file = resolve(repoRoot, path);
    const before = await readFile(file, "utf8");
    const after = (
      path === enginePath
        ? before.replace(constantPattern, (_, head, __, tail) => `${head}${next}${tail}`)
        : before
    ).replace(declaration, (_, head, __, quote) => `${head}${quote}${next}${quote}`);
    if (after !== before) {
      await writeFile(file, after);
      console.log(`Updated ${path}`);
    }
  }
  console.log(`Set Lumen schema version ${current} -> ${next}`);
}
