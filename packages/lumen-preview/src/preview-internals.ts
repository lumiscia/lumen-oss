import type { LumenPreviewError, LumenPreviewState } from "./preview.js";

export interface LumenPreviewTransport {
  pause(): void;
  play(): void;
  seek(frame: number): Promise<boolean>;
  whenReady(): Promise<LumenPreviewError | null>;
}

/** What only a `LumenPreviewSession` may do to the preview it fills in. */
export interface LumenPreviewInternals {
  attach(transport: LumenPreviewTransport): void;
  /** Detaches the transport and resets the state, keeping the error. */
  detach(): void;
  update(patch: Partial<LumenPreviewState>): void;
}

export const previewInternals = new WeakMap<object, LumenPreviewInternals>();

export function internalsOf(preview: object): LumenPreviewInternals {
  const internals = previewInternals.get(preview);
  if (!internals) {
    throw new TypeError("`preview` must come from `createLumenPreview()`.");
  }
  return internals;
}
