import { LumenError, errorFromResponse, isRecord } from "./errors.js";
import type { RenderProgress, RenderStatus } from "./types.js";

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

/** A socket event. `render.started` and `render.queued` carry no progress. */
export type SocketUpdate = Omit<RenderProgress, "progress"> & { readonly progress?: number };

/**
 * How a socket session ended: the render `ended`, the session ran to its `timeout`, or the socket
 * was `dropped` before either. `rejection` is the HTTP error that refused the upgrade, which only
 * a Workers `fetch` upgrade can see.
 */
export interface SocketResult {
  readonly end: "ended" | "timeout" | "dropped";
  readonly rejection?: LumenError;
}

const eventStatuses = new Map<string, RenderStatus>([
  ["render.queued", "queued"],
  ["render.started", "processing"],
  ["render.progress", "processing"],
  ["render.completed", "succeeded"],
  ["render.failed", "failed"],
]);
const statuses = new Set<unknown>(["queued", "processing", "succeeded", "failed"]);

/**
 * Watches one render's socket until the render ends, the socket closes or fails, `timeoutMs` passes
 * or `signal` aborts, calling `onProgress` for each event. The timeout and the signal cover opening
 * the socket too. A socket is only a wake-up: it never rejects, and whatever it reported is
 * confirmed by reading the render.
 */
export function watchRender(options: {
  readonly apiKey: string;
  readonly fetch: typeof fetch;
  readonly onProgress: (update: SocketUpdate) => void;
  readonly signal: AbortSignal | undefined;
  readonly timeoutMs: number;
  /** The socket route as an `http(s)` URL. */
  readonly url: URL;
}): Promise<SocketResult> {
  return new Promise((resolve) => {
    const opening = new AbortController();
    let socket: RenderSocket | undefined;
    let finished = false;

    const end = (result: SocketResult) => {
      if (finished) {
        return;
      }
      finished = true;
      clearTimeout(timer);
      options.signal?.removeEventListener("abort", onAbort);
      opening.abort();
      socket?.close();
      resolve(result);
    };
    const onAbort = () => end({ end: "dropped" });
    const timer = setTimeout(() => end({ end: "timeout" }), options.timeoutMs);
    if (options.signal?.aborted) {
      end({ end: "dropped" });
      return;
    }
    options.signal?.addEventListener("abort", onAbort, { once: true });

    openSocket(options.url, options.apiKey, options.fetch, opening.signal).then(
      (opened) => {
        if (finished) {
          opened.close();
          return;
        }
        socket = opened;
        opened.addEventListener("message", (message) => {
          const update = finished ? undefined : parseEvent(message.data);
          if (update === undefined) {
            return;
          }
          options.onProgress(update);
          if (update.status === "succeeded" || update.status === "failed") {
            end({ end: "ended" });
          }
        });
        opened.addEventListener("close", () => end({ end: "dropped" }));
        opened.addEventListener("error", () => end({ end: "dropped" }));
      },
      (error: unknown) =>
        end({ end: "dropped", ...(error instanceof LumenError ? { rejection: error } : {}) }),
    );
  });
}

/**
 * Opens an authenticated socket. Browsers cannot set headers on a socket, so there the open throws,
 * the wait falls back to reading the render, and the session counts as dropped.
 */
async function openSocket(
  url: URL,
  apiKey: string,
  fetchImpl: typeof fetch,
  signal: AbortSignal,
): Promise<RenderSocket> {
  const headers = { authorization: `Bearer ${apiKey}` };

  if (typeof WebSocketPair !== "undefined") {
    const response = (await fetchImpl(url, {
      headers: { ...headers, upgrade: "websocket" },
      signal,
    })) as Response & { webSocket?: WorkerSocket };
    if (response.webSocket === undefined) {
      throw response.ok
        ? new LumenError("socket_error", "The render socket was not upgraded.")
        : await errorFromResponse(response, signal);
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
 * `{ type, state: { renderId, state, progress } }`, where `state.state` is the render's status.
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

  const typeStatus = eventStatuses.get(message.type);
  const fields = isRecord(message.state) ? message.state : message;
  if (typeStatus === undefined || typeof fields.renderId !== "string") {
    return undefined;
  }
  const status = statuses.has(fields.state) ? (fields.state as RenderStatus) : typeStatus;
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
