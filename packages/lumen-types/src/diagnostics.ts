/** Errors stop a composition rendering; warnings never do. */
export type LumenDiagnosticSeverity = "error" | "warning";

/**
 * A problem the engine found with a composition. `code` is stable and public, and new codes can
 * appear in any release, so treat one you don't know by its severity. `message` is free-form.
 */
export interface LumenDiagnostic {
  readonly severity: LumenDiagnosticSeverity;
  /** Such as `node.unknown_param` or `font.fallback`. */
  readonly code: string;
  /** A JSON pointer into the composition, such as `/nodes/3/params/opacity`; empty for all of it. */
  readonly path: string;
  /** The node's id, in the graph `path` points into. */
  readonly node?: number;
  readonly message: string;
}
