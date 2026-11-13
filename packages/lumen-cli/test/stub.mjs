#!/usr/bin/env node
// Stands in for the lumen binary. STUB_RECORD names the file it writes its arguments and stdin to,
// and STUB_MODE picks what it does after that.
import { writeFileSync } from "node:fs";

const mode = process.env.STUB_MODE;
const record = process.env.STUB_RECORD;

if (mode === "exit-early") {
  process.stderr.write("rejected early\n");
  process.exit(2);
}

if (mode === "hang") {
  writeFileSync(record, JSON.stringify({ pid: process.pid }));
  setInterval(() => {}, 1000);
} else {
  let stdin = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (chunk) => (stdin += chunk));
  process.stdin.on("end", () => {
    writeFileSync(record, JSON.stringify({ args: process.argv.slice(2), stdin }));
    if (mode === "fail") {
      process.stderr.write("lumen failed: boom\n");
      process.exit(3);
    }
    if (mode === "report") {
      process.stdout.write(process.env.STUB_STDOUT);
      process.exit(Number(process.env.STUB_EXIT));
    }
  });
}
