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
export { AudioTrack, Composition } from "@lumiscia/lumen-shared";
export type * from "@lumiscia/lumen-shared";
export type {
  LumenApiError,
  LumenOptions,
  RenderEvent,
  RenderEventHandlers,
  RenderEventSubscription,
  RenderJob,
  RenderMediaManifest,
  RenderOptions,
  RenderResult,
  WaitForRenderOptions,
  LumenMediaReference,
  CompleteMultipartUploadOptions,
  CreateMediaUploadOptions,
  CreateUrlMediaOptions,
  MediaUploadPart,
  TemporaryMedia,
  UploadMediaOptions,
} from "./types.js";
