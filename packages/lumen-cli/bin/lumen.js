#!/usr/bin/env node
import { spawn } from "node:child_process";
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

const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });
// A signal sent to this process alone, as a supervisor does, would otherwise leave lumen running.
const forwarded = ["SIGINT", "SIGTERM", "SIGHUP"];
for (const signal of forwarded) {
  process.on(signal, () => child.kill(signal));
}
child.on("error", (error) => {
  console.error(`lumen: could not run ${binary}: ${error.message}`);
  process.exit(1);
});
child.on("exit", (status, signal) => {
  if (signal) {
    for (const forwardedSignal of forwarded) {
      process.removeAllListeners(forwardedSignal);
    }
    process.kill(process.pid, signal);
  } else {
    process.exit(status ?? 1);
  }
});
