import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { once } from "node:events";
import { readFile } from "node:fs/promises";

import { apiVersion } from "@lumiscia/lumen-types";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";

import { Composition, Lumen, LumenError, assetSource } from "../src/index.js";
import type { RenderProgress } from "../src/types.js";
import { Conformance, type ConformanceProfile } from "./conformance.js";

// Runs against a host that is already up when `LUMEN_API_URL` names it, with `LUMEN_API_TOKEN` and
// optionally `LUMEN_API_PROFILE`, a JSON file of where that host differs from the API on purpose.
// Otherwise `LUMEN_SDK_INTEGRATION=1` starts a lumen-server, which has no profile: it is the API.
const externalUrl = process.env.LUMEN_API_URL;
const shouldRun = externalUrl !== undefined || process.env.LUMEN_SDK_INTEGRATION === "1";
const describeIntegration = shouldRun ? describe : describe.skip;
const token = process.env.LUMEN_API_TOKEN ?? "sdk-integration-token";
const serverFeatures = process.env.LUMEN_SERVER_FEATURES;
const startupTimeoutMs = 180_000;
const renderTimeoutMs = 180_000;

describeIntegration("a Lumen API host, through the SDK", () => {
  let server: ChildProcessWithoutNullStreams | undefined;
  let baseUrl: string;
  let conformance: Conformance;
  let lumen: Lumen;

  beforeAll(async () => {
    if (externalUrl === undefined) {
      server = spawn(
        "cargo",
        [
          "run",
          "-p",
          "lumen-server",
          ...(serverFeatures ? ["--features", serverFeatures] : []),
          "--",
          "--bind",
          "127.0.0.1:0",
          "--token",
          token,
          "--progress-min-delta",
          "0",
        ],
        {
          cwd: new URL("../../..", import.meta.url),
          env: {
            ...process.env,
            NO_COLOR: "1",
            RUST_LOG: process.env.RUST_LOG ?? "lumen_server=info,lumen_engine::render=info,warn",
          },
        },
      );
      baseUrl = `http://${await listeningAddress(server, startupTimeoutMs)}`;
    } else {
      baseUrl = externalUrl;
    }
    const profilePath = process.env.LUMEN_API_PROFILE;
    const profile: ConformanceProfile =
      profilePath === undefined
        ? {}
        : (JSON.parse(await readFile(profilePath, "utf8")) as ConformanceProfile);
    conformance = await Conformance.load(baseUrl, profile);
    lumen = new Lumen({ apiKey: token, baseUrl, fetch: conformance.fetch });
  }, startupTimeoutMs + 5_000);

  afterAll(async () => {
    await stopServer(server);
  });

  // Each test's requests went through the checker, whatever the test itself looked at.
  afterEach(() => {
    expect(conformance.violations.splice(0)).toEqual([]);
  });

  test("says which version of the API it implements", async () => {
    const response = await conformance.fetch(new URL("health", `${baseUrl.replace(/\/$/, "")}/`));
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ ok: true, version: apiVersion });
  });

  test(
    "submits a render, waits for it over the WebSocket, and downloads the artifact",
    async () => {
      const progress: RenderProgress[] = [];
      const created = await lumen.render(tinyComposition());
      expect(created.status).toBe("queued");

      const done = await lumen.waitForRender(created.id, {
        onProgress: (update) => progress.push(update),
      });

      expect(done.status).toBe("succeeded");
      if (externalUrl === undefined) {
        expect(done.url).toBe(`${baseUrl}/renders/${created.id}/artifact`);
      }
      expect(progress.at(-1)).toEqual({ id: created.id, status: "succeeded", progress: 1 });
      expect(await lumen.getRender(created.id)).toMatchObject({ status: "succeeded" });

      const artifact = await (await lumen.download(done)).blob();
      expect(artifact.type).toBe("video/mp4");
      expect(artifact.size).toBeGreaterThan(0);
    },
    renderTimeoutMs,
  );

  test(
    "renders an uploaded file referenced as asset:<id>, then deletes it",
    async () => {
      const upload = await lumen.uploadMedia({
        body: await readFile(new URL("../../../examples/assets/clip.mp4", import.meta.url)),
        fileName: "clip.mp4",
      });

      const composition = new Composition({
        renderSettings: { width: 64, height: 64 },
        timeline: { fps: 24, duration_frames: 1 },
      });
      const clip = composition.addNode({
        type: "media_in",
        params: { kind: "video", source: assetSource(upload) },
      });
      composition.addNode({ type: "media_output", inputs: { source: clip } });

      const created = await lumen.render(composition);
      const done = await lumen.waitForRender(created.id);
      expect(done.status).toBe("succeeded");

      if (conformance.supports("getMedia", "deleteMedia")) {
        expect(await lumen.getMedia(upload)).toMatchObject({ id: upload.id, kind: "video" });
        await lumen.deleteMedia(upload);
        await expect(lumen.getMedia(upload)).rejects.toMatchObject({ code: "media_not_found" });
      }
    },
    renderTimeoutMs,
  );

  test("answers errors with the API's error body and codes", async () => {
    await expect(lumen.getRender("no-such-render")).rejects.toMatchObject({
      code: "render_not_found",
      status: 404,
    });
    const unauthorized = new Lumen({ apiKey: "wrong", baseUrl, fetch: conformance.fetch });
    await expect(unauthorized.getRender("no-such-render")).rejects.toBeInstanceOf(LumenError);
  });

  test("checks a composition without rendering it", async (context) => {
    if (!conformance.supports("checkRender")) {
      context.skip();
    }
    expect(await lumen.check(tinyComposition())).toMatchObject({ ok: true });
    const broken = { ...tinyComposition().toJSON(), nodes: [{ id: "a", type: "nope" }] };
    const report = await lumen.check(broken as never);
    expect(report.ok).toBe(false);
    expect(report.diagnostics.length).toBeGreaterThan(0);
  });

  test("cancels a render", async (context) => {
    if (!conformance.supports("cancelRender")) {
      context.skip();
    }
    const created = await lumen.render(tinyComposition());
    const cancelled = await lumen.cancelRender(created.id);
    expect(["cancelled", "processing", "succeeded"]).toContain(cancelled.status);
  });
});

function tinyComposition(): Composition {
  const size = Number(process.env.LUMEN_SDK_INTEGRATION_SIZE ?? "64");
  const composition = new Composition({
    metadata: {
      name: "SDK integration test",
    },
    renderSettings: {
      width: size,
      height: size,
    },
    timeline: {
      fps: 24,
      duration_frames: 1,
    },
  });
  const background = composition.addNode({
    type: "background",
    params: {
      width: size,
      height: size,
      paint: [255, 64, 32, 255],
    },
  });
  const output = composition.addNode({
    type: "media_output",
  });

  composition.connect(background, output, {
    toPort: "source",
  });

  return composition;
}

/** The address the server logs when it is listening, which is how a bind to port 0 is found out. */
function listeningAddress(
  server: ChildProcessWithoutNullStreams,
  timeoutMs: number,
): Promise<string> {
  return new Promise((resolve, reject) => {
    let output = "";
    const timer = setTimeout(
      () => finish(new Error("Timed out waiting for lumen-server to start.")),
      timeoutMs,
    );
    const finish = (result: string | Error) => {
      clearTimeout(timer);
      server.stdout.off("data", onData);
      server.stderr.off("data", onData);
      server.off("exit", onExit);
      if (typeof result === "string") {
        resolve(result);
      } else {
        reject(result);
      }
    };
    const onData = (chunk: unknown) => {
      output += String(chunk);
      const address = /listening address=(\S+)/.exec(output)?.[1];
      if (address !== undefined) {
        finish(address);
      }
    };
    const onExit = () => finish(new Error("lumen-server exited before startup."));
    server.stdout.on("data", onData);
    server.stderr.on("data", onData);
    server.on("exit", onExit);
  });
}

async function stopServer(server: ChildProcessWithoutNullStreams | undefined): Promise<void> {
  if (!server || server.exitCode !== null) {
    return;
  }

  server.kill("SIGTERM");
  const timeout = setTimeout(() => {
    if (server.exitCode === null) {
      server.kill("SIGKILL");
    }
  }, 5_000);

  try {
    await once(server, "exit");
  } finally {
    clearTimeout(timeout);
  }
}
