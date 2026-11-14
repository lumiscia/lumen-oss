import type { LumenComposition } from "@lumiscia/lumen-types";

import { samePreviewMediaInput, type PreviewMediaEntry } from "../media/dependencies.js";
import type { MediaSourceInput } from "../media/index.js";
import type {
  LumenPreviewComposition,
  LumenPreviewFonts,
  LumenPreviewMedia,
  LumenPreviewMediaEntry,
} from "./types.js";

/** Everything a load needs, ready to compare against the last one and send to the worker. */
export interface ResolvedPreviewInputs {
  composition: LumenPreviewComposition | null | undefined;
  compositionJson: string | null;
  fonts: ReadonlyMap<string, MediaSourceInput>;
  media: ReadonlyMap<string, PreviewMediaEntry>;
}

/**
 * A composition that is the very object `previous` resolved isn't serialized again, so a caller
 * that passes its inputs on every render doesn't pay for the JSON each time. Change a composition
 * by passing a new object.
 */
export function resolvePreviewInputs(
  composition: LumenPreviewComposition | null | undefined,
  media: LumenPreviewMedia = {},
  fonts: LumenPreviewFonts = {},
  previous: ResolvedPreviewInputs | null = null,
): ResolvedPreviewInputs {
  return {
    composition,
    compositionJson:
      previous && previous.composition === composition
        ? previous.compositionJson
        : serializeComposition(composition),
    fonts: new Map(
      Object.entries(fonts).map(([family, source]) => [family, absoluteSource(source)]),
    ),
    media: new Map(
      Object.entries(media).map(([source, value]) => {
        const entry = mediaEntry(value);
        return [source, { ...entry, source: absoluteSource(entry.source) }];
      }),
    ),
  };
}

export function sameMediaEntry(left: PreviewMediaEntry, right: PreviewMediaEntry): boolean {
  return left.kind === right.kind && samePreviewMediaInput(left, right);
}

function serializeComposition(
  composition: LumenPreviewComposition | null | undefined,
): string | null {
  return composition ? JSON.stringify(compositionDocument(composition)) : null;
}

function compositionDocument(composition: LumenPreviewComposition): LumenComposition {
  return "toJSON" in composition && typeof composition.toJSON === "function"
    ? composition.toJSON()
    : (composition as LumenComposition);
}

function mediaEntry(value: LumenPreviewMedia[string]): LumenPreviewMediaEntry {
  return typeof value === "object" && "source" in value ? value : { source: value };
}

/** A worker resolves relative URLs against its own script, which is rarely where the page meant. */
function absoluteSource(source: MediaSourceInput): MediaSourceInput {
  if (typeof source === "string" || source instanceof URL) {
    return absoluteUrl(source);
  }
  if (typeof source === "object" && "kind" in source && source.kind === "url") {
    return typeof source.url === "string" || source.url instanceof URL
      ? { ...source, url: absoluteUrl(source.url) }
      : source;
  }
  return source;
}

export function absoluteUrl(url: string | URL): string {
  return new URL(url, globalThis.document.baseURI).href;
}
