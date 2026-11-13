#!/usr/bin/env node
import { spawn } from "node:child_process";

import { resolveBinary } from "../dist/binary.js";

let binary;
try {
  binary = resolveBinary();
} catch (error) {
  console.error(`lumen: ${error.message}`);
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
