#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";

const platformPackage = `@lumiscia/lumen-cli-${process.platform}-${process.arch}`;

let binary;
try {
  binary = createRequire(import.meta.url).resolve(`${platformPackage}/bin/lumen`);
} catch {
  console.error(
    `lumen: ${platformPackage} is not installed. Prebuilt binaries exist for Linux (glibc) on ` +
      "x64 and arm64 and for macOS on arm64, and are optional dependencies, so an install that " +
      "omits optional dependencies leaves them out. On other systems, build from source: " +
      "https://github.com/lumiscia/lumen-oss/tree/main/crates/cli",
  );
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`lumen: could not run ${binary}: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
