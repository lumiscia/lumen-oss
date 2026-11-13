import { chmodSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import type { LumenComposition } from "@lumiscia/lumen-types";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { checkArgs, previewArgs, renderArgs } from "../src/args.js";
import { LumenCliError, check, preview, render } from "../src/index.js";

const stub = fileURLToPath(new URL("./stub.mjs", import.meta.url));

vi.mock("../src/binary.js", () => ({ resolveBinary: () => stub }));

const composition = { nodes: [], connections: [] } as unknown as LumenComposition;

describe("argument mapping", () => {
  test("render maps every option to its flag", () => {
    expect(
      renderArgs({
        output: "out.mp4",
        frames: { start: 30, end: 90 },
        mediaRoot: "media",
        fontDir: "fonts",
        maxTextGlyphs: 1000,
        codec: "hevc",
        quality: "high",
        qualityConfig: "quality.json",
        bitDepth: 10,
        alpha: false,
        encoder: "libx265",
        bitrate: "14M",
        preset: "slow",
      }),
    ).toEqual([
      "render",
      "-",
      "--output=out.mp4",
      "--frames=30..90",
      "--media-root=media",
      "--font-dir=fonts",
      "--max-text-glyphs=1000",
      "--codec=hevc",
      "--quality=high",
      "--quality-config=quality.json",
      "--bit-depth=10",
      "--encoder=libx265",
      "--bitrate=14M",
      "--preset=slow",
      "--alpha=false",
    ]);
  });

  test("render leaves out what is unset and keeps an open-ended range", () => {
    expect(renderArgs({ output: "a.png", frames: { end: 60 }, alpha: true, frame: 0 })).toEqual([
      "render",
      "-",
      "--output=a.png",
      "--frames=..60",
      "--frame=0",
      "--alpha=true",
    ]);
    expect(renderArgs({ output: "a.png", frames: {} })).toEqual(["render", "-", "--output=a.png"]);
  });

  test("check asks for the JSON report", () => {
    expect(checkArgs({ mediaRoot: "media" })).toEqual([
      "check",
      "-",
      "--format=json",
      "--media-root=media",
    ]);
  });

  test("preview maps its options", () => {
    expect(
      previewArgs({
        output: "sheet.png",
        count: 20,
        columns: 5,
        width: 1600,
        frames: { start: 10 },
      }),
    ).toEqual([
      "preview",
      "-",
      "--output=sheet.png",
      "--count=20",
      "--columns=5",
      "--width=1600",
      "--frames=10..",
    ]);
  });
});

describe("running the binary", () => {
  let dir: string;
  let record: string;

  const recorded = () => JSON.parse(readFileSync(record, "utf8")) as Record<string, unknown>;

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "lumen-cli-test-"));
    record = join(dir, "record.json");
    chmodSync(stub, 0o755);
    vi.stubEnv("STUB_RECORD", record);
  });

  afterEach(() => {
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
    rmSync(dir, { recursive: true, force: true });
  });

  test("render pipes the composition to stdin and closes it", async () => {
    vi.stubEnv("STUB_MODE", "ok");
    const result = await render({ toJSON: () => composition }, { output: "out.png", quiet: true });
    expect(result).toEqual({ output: "out.png" });
    expect(recorded()).toEqual({
      args: ["render", "-", "--output=out.png"],
      stdin: JSON.stringify(composition),
    });
  });

  test("preview pipes the composition to stdin", async () => {
    vi.stubEnv("STUB_MODE", "ok");
    await preview(composition, { output: "sheet.png", count: 4, quiet: true });
    expect(recorded()).toMatchObject({
      args: ["preview", "-", "--output=sheet.png", "--count=4"],
      stdin: JSON.stringify(composition),
    });
  });

  test("a failed run rejects with the exit code and stderr, and passes stderr through unless quiet", async () => {
    vi.stubEnv("STUB_MODE", "fail");
    const written = vi.spyOn(process.stderr, "write").mockReturnValue(true);

    const error = await render(composition, { output: "out.mp4" }).catch(
      (caught: unknown) => caught,
    );
    expect(error).toBeInstanceOf(LumenCliError);
    expect(error).toMatchObject({ exitCode: 3, signal: null, stderr: "lumen failed: boom\n" });
    expect(written).toHaveBeenCalledWith("lumen failed: boom\n");

    written.mockClear();
    await expect(render(composition, { output: "out.mp4", quiet: true })).rejects.toMatchObject({
      stderr: "lumen failed: boom\n",
    });
    expect(written).not.toHaveBeenCalled();
  });

  test("a binary that exits before reading its input rejects without an unhandled error", async () => {
    vi.stubEnv("STUB_MODE", "exit-early");
    const large = { padding: "x".repeat(8_000_000) } as unknown as LumenComposition;
    await expect(render(large, { output: "out.mp4", quiet: true })).rejects.toMatchObject({
      exitCode: 2,
      stderr: "rejected early\n",
    });
  });

  test("check resolves with the report for a composition with errors", async () => {
    const report = {
      ok: false,
      diagnostics: [
        { severity: "error", code: "node.unknown_param", path: "/nodes/0", message: "bad" },
      ],
    };
    vi.stubEnv("STUB_MODE", "report");
    vi.stubEnv("STUB_STDOUT", JSON.stringify(report));
    vi.stubEnv("STUB_EXIT", "1");
    expect(await check(composition)).toEqual(report);
    expect(recorded()).toMatchObject({ args: ["check", "-", "--format=json"] });
  });

  test("check rejects when exit 1 comes without a report", async () => {
    vi.stubEnv("STUB_MODE", "report");
    vi.stubEnv("STUB_STDOUT", "");
    vi.stubEnv("STUB_EXIT", "1");
    await expect(check(composition)).rejects.toMatchObject({ exitCode: 1 });
  });

  test("aborting kills the child and rejects with the signal's reason", async () => {
    vi.stubEnv("STUB_MODE", "hang");
    const controller = new AbortController();
    const pending = render(composition, { output: "out.mp4", signal: controller.signal }).catch(
      (caught: unknown) => caught,
    );
    const { pid } = await vi.waitFor(() => recorded() as { pid: number });
    controller.abort();
    expect(await pending).toBe(controller.signal.reason);
    expect(() => process.kill(pid, 0)).toThrow();
  });
});
