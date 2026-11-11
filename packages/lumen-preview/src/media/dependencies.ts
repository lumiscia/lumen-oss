import type {
  MediaFailure,
  MediaInventory,
  MediaLocation,
  MediaSourceKind,
} from "@lumiscia/lumen-types";
import type { InputFormat } from "mediabunny";

import type { MediaDependency } from "./lifecycle.js";
import { sameMediaInput } from "./source.js";
import type { MediaSourceInput } from "./types.js";

/** What the host passes for a source in `media`, with its source made absolute. */
export interface PreviewMediaEntry {
  fps?: number | null;
  formats?: InputFormat[];
  kind?: MediaSourceKind;
  source: MediaSourceInput;
  track?: number | "primary";
}

/** What a dependency loads: the host's source, with how to read it for a video. */
export interface PreviewMediaInput {
  fps?: number | null;
  formats?: InputFormat[];
  source: MediaSourceInput;
  track?: number | "primary";
}

export type PreviewMediaDependency = MediaDependency<PreviewMediaInput>;

/**
 * Why a source is refused whatever the host passes, as `lumen-server` refuses it before looking in
 * its media root: a remote URL, a URI scheme other than `file://`, an empty path or one that goes
 * up with `..`. Null for a source that is looked up, by its name in `media` or as an `asset:` id.
 */
export function refusedSource(source: string): string | null {
  if (source.startsWith("asset:")) {
    return null;
  }
  if (/^https?:\/\//i.test(source)) {
    return "remote media sources aren't read: name the source, and pass its URL in `media` under that name";
  }
  if (source.includes("://") && !source.startsWith("file://")) {
    return "this URI scheme isn't a media source";
  }
  const path = source.startsWith("file://") ? source.slice("file://".length) : source;
  if (path === "") {
    return "the path is empty";
  }
  if (path.split("/").includes("..")) {
    return "a source may not go up a directory with `..`";
  }
  return null;
}

/**
 * The dependencies of a composition, from what the engine lists it needing and what the host
 * passes: one for each source and kind it is read as, and one for each font. The visual kinds of
 * one use are alternatives, of which the entry's kind picks; its audio, and the kinds of other
 * uses, are each needed. A source an expression computes can be any entry of `media`, so each
 * entry is also read as the kind such a use reads, unless its `kind` or blob type says it holds
 * another or a literal use already reads it as one of them.
 */
export function previewDependencies(
  inventory: MediaInventory,
  media: ReadonlyMap<string, PreviewMediaEntry>,
  fonts: ReadonlyMap<string, MediaSourceInput>,
): PreviewMediaDependency[] {
  const dependencies = new Map<string, PreviewMediaDependency>();
  const add = (
    source: string,
    kind: MediaSourceKind,
    at: MediaLocation | undefined,
    wanted: readonly MediaSourceKind[],
  ) => {
    const added = dependency(source, kind, at, wanted, media.get(source));
    if (!dependencies.has(added.key)) {
      dependencies.set(added.key, added);
    }
  };

  for (const { source, uses } of inventory.media) {
    for (const { at, kinds } of uses) {
      if (kinds.includes("audio")) {
        add(source, "audio", at, ["audio"]);
      }
      const visual = kinds.filter((kind) => kind !== "audio");
      const kind = entryKind(media.get(source), visual) ?? visual[0];
      if (kind) {
        add(source, kind, at, visual);
      }
    }
  }

  for (const { kinds, ...param } of inventory.dynamic) {
    const at: MediaLocation = { type: "node", ...param };
    for (const [source, entry] of media) {
      const declared = entry.kind ?? sourceKind(entry.source);
      if (declared !== undefined && !kinds.includes(declared)) {
        continue;
      }
      const kind = declared ?? (kinds.length === 1 ? kinds[0] : undefined);
      if (!kind && kinds.some((read) => dependencies.has(`${read}:${source}`))) {
        continue;
      }
      if (!kind) {
        throw new Error(
          `media "${source}" needs a kind: an expression can name it, and its entry and source don't say what it holds`,
        );
      }
      add(source, kind, at, kinds);
    }
  }

  for (const [family, source] of fonts) {
    dependencies.set(`font:${family}`, {
      key: `font:${family}`,
      kind: "font",
      load: { input: { source } },
      source: family,
    });
  }
  return [...dependencies.values()];
}

export function samePreviewMediaInput(left: PreviewMediaInput, right: PreviewMediaInput): boolean {
  return (
    sameMediaInput(left.source, right.source) &&
    left.fps === right.fps &&
    left.track === right.track &&
    sameFormats(left.formats, right.formats)
  );
}

function dependency(
  source: string,
  kind: MediaSourceKind,
  at: MediaLocation | undefined,
  wanted: readonly MediaSourceKind[],
  entry: PreviewMediaEntry | undefined,
): PreviewMediaDependency {
  const base = { key: `${kind}:${source}`, kind, source, ...(at && { at }) };
  const fail = (failure: Omit<MediaFailure, "source" | "wanted">) => ({
    ...base,
    load: { failure: { source, wanted, ...(at && { at }), ...failure } },
  });
  const refused = refusedSource(source);
  if (refused !== null) {
    return fail({ code: "media.not_found", details: refused });
  }
  if (!entry) {
    return fail({
      code: "media.not_found",
      details: source.startsWith("asset:")
        ? "the preview has no asset store: pass this asset's URL in `media` under its `asset:` id"
        : "`media` has no entry by this name",
    });
  }
  if (entry.kind !== undefined && !wanted.includes(entry.kind) && kind !== "audio") {
    return fail({
      code: "media.wrong_kind",
      details: `its \`media\` entry says it is ${entry.kind}`,
      found: entry.kind,
    });
  }
  const { fps, formats, source: input, track } = entry;
  return {
    ...base,
    load: {
      input: {
        source: input,
        ...(fps !== undefined && { fps }),
        ...(formats !== undefined && { formats }),
        ...(track !== undefined && { track }),
      },
    },
  };
}

/** The kind an entry says it holds, by its `kind` or its blob's type, if `kinds` reads it. */
function entryKind(
  entry: PreviewMediaEntry | undefined,
  kinds: readonly MediaSourceKind[],
): MediaSourceKind | undefined {
  const kind = entry?.kind ?? (entry && sourceKind(entry.source));
  return kind !== undefined && kinds.includes(kind) ? kind : undefined;
}

/** The kind a source says it holds: by its blob's type, or a `.cube` file name or URL path. */
function sourceKind(source: MediaSourceInput): MediaSourceKind | undefined {
  if (isCubeName(source)) {
    return "lut";
  }
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

function isCubeName(source: MediaSourceInput): boolean {
  // A query or fragment that happens to end in `.cube` names nothing.
  return /\.cube$/i.test(sourceName(source).split(/[?#]/, 1)[0] ?? "");
}

/** The file name or URL a source carries, or an empty string for bytes. */
function sourceName(source: MediaSourceInput): string {
  if (typeof source === "string") {
    return source;
  }
  if (source instanceof URL) {
    return source.pathname;
  }
  if (source instanceof Request) {
    return source.url;
  }
  if (source instanceof File) {
    return source.name;
  }
  if (typeof source === "object" && "kind" in source) {
    return source.kind === "url"
      ? sourceName(source.url)
      : source.kind === "blob"
        ? sourceName(source.blob)
        : "";
  }
  return "";
}

function sameFormats(left: InputFormat[] | undefined, right: InputFormat[] | undefined): boolean {
  if (left === right) {
    return true;
  }
  if (!left || !right || left.length !== right.length) {
    return false;
  }
  return left.every((format, index) => format === right[index]);
}
