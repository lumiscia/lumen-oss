import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { once } from "node:events";
import { readFile } from "node:fs/promises";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { Composition, Lumen, assetSource } from "../src/index.js";
import type { RenderProgress } from "../src/types.js";

const shouldRun = process.env.LUMEN_SDK_INTEGRATION === "1";
const describeIntegration = shouldRun ? describe : describe.skip;
const serverToken = "sdk-integration-token";
const serverFeatures = process.env.LUMEN_SERVER_FEATURES ?? "cli";
const startupTimeoutMs = 180_000;
const renderTimeoutMs = 180_000;

describeIntegration("Lumen SDK with lumen-server", () => {
  let server: ChildProcessWithoutNullStreams | undefined;
  let baseUrl: string;
  let serverOutput = "";

  beforeAll(async () => {
    server = spawn(
      "cargo",
      [
        "run",
        "-p",
        "lumen-server",
        "--features",
        serverFeatures,
        "--bin",
        "lumen-server",
        "--",
        "--bind",
        "127.0.0.1:0",
        "--token",
        serverToken,
        "--progress-min-delta",
        "0",
      ],
      {
        cwd: new URL("../../..", import.meta.url),
        env: {
          ...process.env,
          NO_COLOR: "1",
          RUST_LOG: process.env.RUST_LOG ?? "lumen_server=info,warn",
        },
      },
    );
    const listening = listeningAddress(server, startupTimeoutMs);
    for (const stream of [server.stdout, server.stderr]) {
      stream.on("data", (chunk) => {
        serverOutput += String(chunk);
      });
    }

    baseUrl = `http://${await listening}`;
  }, startupTimeoutMs + 5_000);

  afterAll(async () => {
    await stopServer(server);
  });

  test(
    "submits a render, waits for it over the WebSocket, and downloads the artifact",
    async () => {
      const lumen = new Lumen({
        apiKey: serverToken,
        baseUrl,
      });
      const progress: RenderProgress[] = [];
      const composition = tinyComposition();

      const created = await lumen.render(composition, {
        idempotencyKey: "sdk-integration-solid-color",
      });
      expect(created.status).toBe("queued");

      const done = await lumen.waitForRender(created.id, {
        onProgress: (update) => progress.push(update),
      });

      expect(done.status).toBe("succeeded");
      expect(done.url).toBe(`${baseUrl}/renders/${created.id}/artifact`);
      expect(progress.at(-1)).toEqual({ id: created.id, status: "succeeded", progress: 1 });

      const artifact = await (await lumen.download(done)).blob();
      expect(artifact.type).toBe("video/mp4");
      expect(artifact.size).toBeGreaterThan(0);
    },
    renderTimeoutMs,
  );

  test(
    "renders an uploaded file referenced as asset:<id>",
    async () => {
      const lumen = new Lumen({ apiKey: serverToken, baseUrl });
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
    },
    renderTimeoutMs,
  );
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
