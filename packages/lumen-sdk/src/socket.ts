import { isRecord } from "./errors.js";
import type { RenderProgress, RenderStatus } from "./types.js";

/** A socket event. `render.started` and `render.queued` carry no progress. */
export type SocketUpdate = Omit<RenderProgress, "progress"> & { readonly progress?: number };

/** Present in Cloudflare Workers, where a socket comes from a `fetch` upgrade. */
declare const WebSocketPair: unknown;

interface RenderSocket {
  addEventListener(type: "close" | "error", listener: () => void): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
  close(): void;
}

interface WorkerSocket extends RenderSocket {
  accept(): void;
}

/**
 * How a socket session ended: the render `ended`, the session ran to its `timeout`, or the socket
 * was `dropped` before either.
 */
export type SocketSession = "ended" | "timeout" | "dropped";

const eventStatuses = new Map<string, RenderStatus>([
  ["render.queued", "queued"],
  ["render.started", "processing"],
  ["render.progress", "processing"],
  ["render.completed", "succeeded"],
  ["render.failed", "failed"],
]);

/**
 * Watches one render's socket until the render ends, the socket closes or fails, `timeoutMs` passes
 * or `signal` aborts, calling `onProgress` for each event. A socket is only a wake-up: it never
 * rejects, and whatever it reported is confirmed by reading the render.
 */
export async function watchRender(options: {
  readonly apiKey: string;
  readonly fetch: typeof fetch;
  readonly onProgress: (update: SocketUpdate) => void;
  readonly signal: AbortSignal | undefined;
  readonly timeoutMs: number;
  /** The socket route as an `http(s)` URL. */
  readonly url: URL;
}): Promise<SocketSession> {
  let socket: RenderSocket;
  try {
    socket = await openSocket(options.url, options.apiKey, options.fetch);
  } catch {
    return "dropped";
  }

  return new Promise((resolve) => {
    const end = (session: SocketSession) => {
      clearTimeout(timer);
      options.signal?.removeEventListener("abort", onAbort);
      socket.close();
      resolve(session);
    };
    const onAbort = () => end("dropped");
    const timer = setTimeout(() => end("timeout"), options.timeoutMs);
    options.signal?.addEventListener("abort", onAbort, { once: true });

    socket.addEventListener("message", (message) => {
      const progress = parseEvent(message.data);
      if (progress === undefined) {
        return;
      }
      options.onProgress(progress);
      if (progress.status === "succeeded" || progress.status === "failed") {
        end("ended");
      }
    });
    socket.addEventListener("close", () => end("dropped"));
    socket.addEventListener("error", () => end("dropped"));
  });
}

/**
 * Opens an authenticated socket. Browsers cannot set headers on a socket, so there the open throws,
 * the wait falls back to reading the render, and the session counts as ended early.
 */
async function openSocket(
  url: URL,
  apiKey: string,
  fetchImpl: typeof fetch,
): Promise<RenderSocket> {
  const headers = { authorization: `Bearer ${apiKey}` };

  if (typeof WebSocketPair !== "undefined") {
    const response = (await fetchImpl(url, {
      headers: { ...headers, upgrade: "websocket" },
    })) as Response & { webSocket?: WorkerSocket };
    if (response.webSocket === undefined) {
      throw new Error(`Render socket was rejected with HTTP ${response.status}.`);
    }
    response.webSocket.accept();
    return response.webSocket;
  }

  const socketUrl = new URL(url);
  socketUrl.protocol = url.protocol === "http:" ? "ws:" : "wss:";
  return new WebSocket(socketUrl, { headers } as unknown as string[]);
}

/**
 * Reads a socket message: `{ type, renderId, progress }` from lumen-server, or the hosted API's
 * `{ type, state: { renderId, progress } }`.
 */
function parseEvent(data: unknown): SocketUpdate | undefined {
  let message: unknown;
  try {
    message = JSON.parse(String(data));
  } catch {
    return undefined;
  }
  if (!isRecord(message) || typeof message.type !== "string") {
    return undefined;
  }

  const status = eventStatuses.get(message.type);
  const fields = isRecord(message.state) ? message.state : message;
  if (status === undefined || typeof fields.renderId !== "string") {
    return undefined;
  }
  return {
    id: fields.renderId,
    status,
    ...(status === "succeeded"
      ? { progress: 1 }
      : typeof fields.progress === "number"
        ? { progress: fields.progress }
        : {}),
  };
}
