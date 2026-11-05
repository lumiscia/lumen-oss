import WebSocket from "ws";

import { Lumen as BaseLumen, mediaReference } from "./client.js";
import type { LumenOptions } from "./types.js";

export class Lumen extends BaseLumen {
  constructor(options: LumenOptions) {
    super({
      ...options,
      websocket:
        options.websocket ??
        ((url, websocketOptions) =>
          new WebSocket(url, {
            headers: Object.fromEntries(websocketOptions.headers),
          })),
    });
  }
}

export { mediaReference };
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
