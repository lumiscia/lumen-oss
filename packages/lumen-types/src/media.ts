import type { MediaSourceKind } from "./generated/meta.js";

/** A param of a node, in the root composition or in one of its `compositions` templates. */
export interface NodeParamLocation {
  /** The template the node is in; absent for the root. */
  readonly template?: string;
  readonly node: number;
  /** The param's path in the node's `params`, such as `source` or `content[1].font_family`. */
  readonly param: string;
}

/** Where a composition names a source. */
export type MediaLocation =
  | ({ readonly type: "node" } & NodeParamLocation)
  | {
      readonly type: "audioClip";
      readonly template?: string;
      readonly track: string;
      readonly clip: string;
      /** The clip's position in the timeline's `clips`. */
      readonly index: number;
    };

/** One place a source is used, and the kinds of media it reads there. */
export interface MediaUse {
  readonly at: MediaLocation;
  readonly kinds: readonly MediaSourceKind[];
}

/**
 * A source a composition names as written (a path, a URL or an alias, or `asset:<id>`), with each
 * place it is used.
 */
export interface MediaDependency {
  readonly source: string;
  /** The kinds the uses read, without repeats, in the order the uses first name them. */
  readonly kinds: readonly MediaSourceKind[];
  readonly uses: readonly MediaUse[];
}

/** A media param set by an expression, whose source is known only when a frame evaluates it. */
export interface DynamicMediaUse extends NodeParamLocation {
  readonly kinds: readonly MediaSourceKind[];
}

/**
 * A font family a text node names as a literal. A family is not a media source: a host resolves
 * it by name, falling back to its own fonts, unless it is written `asset:<id>`.
 */
export interface FontDependency {
  readonly family: string;
  readonly uses: readonly NodeParamLocation[];
}

/** Everything a composition needs from its host before it renders. */
export interface MediaInventory {
  /** Each distinct source, in the order the nodes and then the audio clips first name it. */
  readonly media: readonly MediaDependency[];
  readonly dynamic: readonly DynamicMediaUse[];
  readonly fonts: readonly FontDependency[];
}

/** A kind of media a failure can name: a source kind, or `font` for font files. */
export type MediaFailureKind = MediaSourceKind | "font";

/**
 * Why a media source can't be used, with its public diagnostic code. `media.decode` is data that
 * should be what the use reads but is corrupt, truncated or in no format the engine identifies;
 * `media.unsupported` is data it identifies and doesn't read, such as a codec it lacks.
 */
export type MediaFailureCode =
  | "media.decode"
  | "media.not_found"
  | "media.too_large"
  | "media.unsupported"
  | "media.wrong_kind";

/** A media source that can't be used, the same way for every kind and every host. */
export interface MediaFailure {
  readonly code: MediaFailureCode;
  /** The source as the composition wrote it: a path, URL, alias or `asset:<id>`. */
  readonly source: string;
  /** The kinds the place using the source reads; empty when the host that failed doesn't know. */
  readonly wanted: readonly MediaFailureKind[];
  /** The kind the source is, when it is known to be another than `wanted`. */
  readonly found?: MediaFailureKind;
  /** Where the composition uses the source, when known. */
  readonly at?: MediaLocation;
  readonly details: string;
  /** For `media.too_large`, the limit, in the unit `details` names. */
  readonly limit?: number;
  /** For `media.too_large`, the value over the limit, when it is known. */
  readonly actual?: number;
}
