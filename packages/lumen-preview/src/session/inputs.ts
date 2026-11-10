import type { LumenComposition } from "@lumiscia/lumen-types";

import type { AudioSourceRegistration } from "../audio-engine.js";
import type { MediaRegistration, MediaSourceInput } from "../media/index.js";
import { sameMediaInput } from "../media/source.js";
import type {
  LumenPreviewComposition,
  LumenPreviewFonts,
  LumenPreviewMedia,
  LumenPreviewMediaEntry,
  LumenPreviewMediaKind,
} from "./types.js";

/** Everything a load needs, ready to compare against the last one and send to the worker. */
export interface ResolvedPreviewInputs {
  audio: readonly AudioSourceRegistration[];
  compositionJson: string | null;
  fonts: ReadonlyMap<string, MediaSourceInput>;
  media: ReadonlyMap<string, MediaRegistration>;
}

export function resolvePreviewInputs(
  composition: LumenPreviewComposition | null | undefined,
  media: LumenPreviewMedia = {},
  fonts: LumenPreviewFonts = {},
): ResolvedPreviewInputs {
  const document = composition ? compositionDocument(composition) : null;
  const audioAliases = new Set(document?.audio?.clips.map((clip) => clip.source_id));
  const sourceKinds = literalSourceKinds(document);
  const audio: AudioSourceRegistration[] = [];
  const visual = new Map<string, MediaRegistration>();

  for (const [id, value] of Object.entries(media)) {
    const { kind: declaredKind, source: rawSource, ...options } = mediaEntry(value);
    const source = absoluteSource(rawSource);
    const kind = declaredKind ?? blobKind(source) ?? sourceKinds.get(id);
    if (audioAliases.has(id)) {
      audio.push({ id, kind: "audio", source });
    }
    if (kind === "image" || kind === "model") {
      visual.set(id, { id, kind, source });
    } else if (kind === "video") {
      visual.set(id, { id, kind, source, ...options });
    } else if (kind === undefined && !audioAliases.has(id)) {
      throw new Error(
        `media "${id}" needs a kind: no media_in node or audio clip names it, and its source doesn't say what it holds`,
      );
    }
  }

  return {
    audio,
    compositionJson: document && JSON.stringify(document),
    fonts: new Map(
      Object.entries(fonts).map(([family, source]) => [family, absoluteSource(source)]),
    ),
    media: visual,
  };
}

export function sameRegistration(left: MediaRegistration, right: MediaRegistration): boolean {
  if (left.kind !== right.kind || !sameMediaInput(left.source, right.source)) {
    return false;
  }
  return (
    left.kind !== "video" ||
    right.kind !== "video" ||
    (left.fps === right.fps && left.track === right.track && left.formats === right.formats)
  );
}

export function sameAudioSources(
  left: readonly AudioSourceRegistration[],
  right: readonly AudioSourceRegistration[],
): boolean {
  return (
    left.length === right.length &&
    left.every(
      (source, index) =>
        source.id === right[index]?.id && sameMediaInput(source.source, right[index].source),
    )
  );
}

function compositionDocument(composition: LumenPreviewComposition): LumenComposition {
  return "toJSON" in composition && typeof composition.toJSON === "function"
    ? composition.toJSON()
    : (composition as LumenComposition);
}

function mediaEntry(value: LumenPreviewMedia[string]): LumenPreviewMediaEntry {
  return typeof value === "object" && "source" in value ? value : { source: value };
}

/**
 * What each literal `source` reads: a model for `model_3d`, and for `media_in` its `kind`, an image
 * without one, as the engine does.
 */
function literalSourceKinds(document: LumenComposition | null): Map<string, LumenPreviewMediaKind> {
  const kinds = new Map<string, LumenPreviewMediaKind>();
  for (const node of document?.nodes ?? []) {
    if (node.type !== "media_in" && node.type !== "model_3d") {
      continue;
    }
    const params = (node.params ?? {}) as { kind?: unknown; source?: unknown };
    if (typeof params.source === "string") {
      kinds.set(
        params.source,
        node.type === "model_3d" ? "model" : params.kind === "video" ? "video" : "image",
      );
    }
  }
  return kinds;
}

function blobKind(source: MediaSourceInput): LumenPreviewMediaKind | undefined {
  const blob =
    source instanceof Blob
      ? source
      : typeof source === "object" && "kind" in source && source.kind === "blob"
        ? source.blob
        : null;
  if (blob?.type === "model/gltf-binary") {
    return "model";
  }
  const group = blob?.type.split("/")[0];
  return group === "audio" || group === "image" || group === "video" ? group : undefined;
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
