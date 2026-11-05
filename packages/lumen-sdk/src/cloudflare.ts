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
export { LumenError } from "./errors.js";
export { LumenWebhookError, verifyWebhook } from "./webhooks.js";
export type {
  LumenWebhookErrorCode,
  RenderWebhookEvent,
  VerifyWebhookOptions,
  WebhookHeaders,
} from "./webhooks.js";
export { Composition } from "@lumiscia/lumen-shared";
export type * from "@lumiscia/lumen-shared";
export type {
  CompleteMultipartUploadOptions,
  CreateMediaUploadOptions,
  CreateMediaUploadResult,
  CreateMultipartUploadResult,
  CreateUrlMediaOptions,
  LumenMediaReference,
  LumenOptions,
  LumenWebSocket,
  LumenWebSocketFactory,
  LumenWebSocketFactoryOptions,
  MediaUploadPart,
  MultipartUploadOptions,
  Render,
  RenderEvent,
  RenderEventHandlers,
  RenderEventSubscription,
  RenderFailure,
  RenderMediaManifest,
  RenderOptions,
  RenderStatus,
  TemporaryMedia,
  UploadMediaBody,
  UploadMediaOptions,
  WaitForRenderOptions,
} from "./types.js";
