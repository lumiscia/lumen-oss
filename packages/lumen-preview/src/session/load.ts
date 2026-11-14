import type { MediaEntryState } from "../media/lifecycle.js";
import type { ResolvedPreviewInputs } from "./inputs.js";

/**
 * How far one load has got. A load has no composition (`empty`), is waiting for the worker
 * (`loading`), has been loaded by it (`loaded`), has had a frame of it drawn (`shown`), or has
 * failed, also after it was loaded.
 */
export type LoadStage = "empty" | "loading" | "loaded" | "shown" | "failed";

/** One load of the inputs the worker reads. The next load replaces it, so nothing carries over. */
export class Load {
  stage: LoadStage;
  // What was last asked for, whether or not it worked: only a change is worth trying again.
  inputs: ResolvedPreviewInputs;
  // What the worker last said of this load's media.
  media: readonly MediaEntryState[] | null = null;

  constructor(
    readonly id: number,
    inputs: ResolvedPreviewInputs,
  ) {
    this.inputs = inputs;
    this.stage = inputs.compositionJson === null ? "empty" : "loading";
  }

  /** A seek waits for the load, to be clamped to the new length. */
  get isLoading(): boolean {
    return this.stage === "loading";
  }

  get mediaPending(): boolean {
    return this.media?.some(({ status }) => status === "pending") ?? false;
  }

  /** Whether the load is loaded and drawn, which is when `whenReady` can settle. */
  get isShown(): boolean {
    return this.stage === "shown";
  }

  loaded(): void {
    this.stage = "loaded";
  }

  failed(): void {
    this.stage = "failed";
  }

  /** Returns whether this is the first frame of the load to be drawn. */
  shown(): boolean {
    if (this.stage !== "loaded") {
      return false;
    }
    this.stage = "shown";
    return true;
  }
}
