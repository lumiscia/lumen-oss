import { Lumen as BaseLumen, mediaReference } from "./client.js";
import type { LumenOptions, LumenWebSocket } from "./types.js";

export class Lumen extends BaseLumen {
  constructor(options: LumenOptions) {
    super({
      ...options,
      websocket:
        options.websocket ??
        (async (url, websocketOptions) => {
          const headers = new Headers(websocketOptions.headers);
          headers.set("upgrade", "websocket");

          const response = (await (options.fetch ?? globalThis.fetch)(url, {
            headers,
            method: "GET",
          })) as Response & { webSocket?: WebSocket };
          if (!response.webSocket) {
            throw new Error(`Lumen render WebSocket was rejected with HTTP ${response.status}.`);
          }
          const socket = response.webSocket as LumenWebSocket;
          socket.accept?.();
          return socket;
        }),
    });
  }
}

export { mediaReference };
export { Composition } from "@lumiscia/lumen-shared";
export type * from "@lumiscia/lumen-shared";
export type {
  CompleteMultipartUploadOptions,
  CreateMediaUploadOptions,
  CreateUrlMediaOptions,
  LumenApiError,
  LumenMediaReference,
  LumenOptions,
  MediaUploadPart,
  RenderEvent,
  RenderEventHandlers,
  RenderEventSubscription,
  RenderJob,
  RenderMediaManifest,
  RenderOptions,
  RenderResult,
  TemporaryMedia,
  UploadMediaOptions,
} from "./types.js";
