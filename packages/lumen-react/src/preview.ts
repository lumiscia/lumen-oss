import { useSyncExternalStore } from "react";

export {
  LumenPreviewContext,
  createLumenPreview,
  type LumenPreviewErrorCode,
  type LumenPreviewListener,
  type LumenPreviewState,
} from "@lumiscia/lumen-preview/preview";

import type { LumenPreviewContext, LumenPreviewState } from "@lumiscia/lumen-preview/preview";

/**
 * The preview's state, re-rendering the component on every change to it. `frame` changes on every
 * played frame, so a component that needs only one part of the state should pass `select`:
 *
 * ```tsx
 * const isPlaying = useLumenPreview(preview, (state) => state.isPlaying);
 * ```
 *
 * The component then re-renders only when the selected value changes. `select` must return a
 * field of the state or a primitive computed from it. React compares the results with `Object.is`,
 * so a selector that builds a new object or array on each call never settles, and React throws
 * "Maximum update depth exceeded".
 */
export function useLumenPreview(preview: LumenPreviewContext): LumenPreviewState;
export function useLumenPreview<T>(
  preview: LumenPreviewContext,
  select: (state: LumenPreviewState) => T,
): T;
export function useLumenPreview<T>(
  preview: LumenPreviewContext,
  select?: (state: LumenPreviewState) => T,
): LumenPreviewState | T {
  const getSnapshot = () => (select ? select(preview.getSnapshot()) : preview.getSnapshot());
  return useSyncExternalStore(preview.subscribe, getSnapshot, getSnapshot);
}
