import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { once } from "node:events";
import net from "node:net";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { Composition, Lumen } from "../src/index.js";
import type { RenderEvent } from "../src/types.js";

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
    const port = await freePort();
    baseUrl = `http://127.0.0.1:${port}`;
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
        `127.0.0.1:${port}`,
        "--token",
        serverToken,
        "--progress-min-delta",
        "0",
      ],
      {
        cwd: new URL("../../..", import.meta.url),
        env: {
          ...process.env,
          RUST_LOG: process.env.RUST_LOG ?? "lumen_server=info,warn",
        },
      },
    );
    server.stdout.on("data", (chunk) => {
      serverOutput += String(chunk);
    });
    server.stderr.on("data", (chunk) => {
      serverOutput += String(chunk);
    });

    await waitForServer(baseUrl, server, startupTimeoutMs);
  }, startupTimeoutMs + 5_000);

  afterAll(async () => {
    await stopServer(server);
  });

  test(
    "submits a render, receives WebSocket completion, and downloads the artifact",
    async () => {
      const lumen = new Lumen({
        apiKey: serverToken,
        baseUrl,
      });
      const events: RenderEvent[] = [];
      const composition = tinyComposition();

      const created = await lumen.render(composition, {
        idempotencyKey: "sdk-integration-solid-color",
      });

      if (created.error !== undefined) {
        throw new Error(`${created.error}\n\nlumen-server output:\n${serverOutput}`);
      }
      expect(created.id).toBeDefined();
      expect(created.render?.status).toBe("queued");

      const completed = await lumen.waitForRender(created.id!, {
        onEvent: (event) => events.push(event),
      });

      expect(completed.type).toBe("render.completed");
      expect(completed.renderId).toBe(created.id);
      expect(completed.url).toBe(`/renders/${created.id}/artifact`);
      expect(events.at(-1)).toEqual(completed);

      const fetched = await lumen.getRender(created.id!);
      if (fetched.error !== undefined) {
        throw new Error(`${fetched.error}\n\nlumen-server output:\n${serverOutput}`);
      }
      expect(fetched.render?.status).toBe("succeeded");

      const artifact = await lumen.getRenderArtifact(created.id!);
      expect(artifact.type).toBe("video/mp4");
      expect(artifact.size).toBeGreaterThan(0);
    },
    renderTimeoutMs,
  );
});

function tinyComposition(): Composition {
  const composition = new Composition({
    metadata: {
      name: "SDK integration test",
    },
    renderSettings: {
      width: 64,
      height: 64,
      background_color: [0, 0, 0, 255],
    },
    timeline: {
      fps: 24,
      duration_frames: 1,
    },
  });
  const background = composition.addNode({
    type: "background",
    params: {
      width: 64,
      height: 64,
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

async function freePort(): Promise<number> {
  const server = net.createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  server.close();
  await once(server, "close");

  if (typeof address !== "object" || address === null) {
    throw new Error("Could not allocate a TCP port for lumen-server.");
  }

  return address.port;
}

async function waitForServer(
  baseUrl: string,
  server: ChildProcessWithoutNullStreams,
  timeoutMs: number,
): Promise<void> {
  const startedAt = Date.now();

  while (Date.now() - startedAt < timeoutMs) {
    if (server.exitCode !== null) {
      throw new Error("lumen-server exited before startup.");
    }

    try {
      const response = await fetch(new URL("/health", baseUrl));
      if (response.ok) {
        return;
      }
    } catch {
      // The server is still compiling or binding its socket.
    }

    await sleep(250);
  }

  throw new Error("Timed out waiting for lumen-server to start.");
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

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
