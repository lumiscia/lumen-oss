import { useSyncExternalStore } from "react";

export {
  LumenPreviewContext,
  createLumenPreview,
  type LumenPreviewErrorCode,
  type LumenPreviewListener,
  type LumenPreviewState,
} from "@lumiscia/lumen-preview/preview";

import type { LumenPreviewContext, LumenPreviewState } from "@lumiscia/lumen-preview/preview";

export function useLumenPreview(preview: LumenPreviewContext): LumenPreviewState {
  return useSyncExternalStore(preview.subscribe, preview.getSnapshot, preview.getSnapshot);
}
