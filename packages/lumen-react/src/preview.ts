import { useSyncExternalStore } from "react";

export {
  LumenPreviewContext,
  createLumenPreview,
  type LumenPreviewListener,
  type LumenPreviewPatch,
  type LumenPreviewState,
  type LumenPreviewTransport,
} from "lumen-preview/preview";

import type { LumenPreviewContext, LumenPreviewState } from "lumen-preview/preview";

export function useLumenPreview(preview: LumenPreviewContext): LumenPreviewState {
  return useSyncExternalStore(preview.subscribe, preview.getSnapshot, preview.getSnapshot);
}
