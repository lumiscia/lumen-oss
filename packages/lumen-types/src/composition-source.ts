import type { LumenComposition } from "./generated/schema.js";

/** A composition as JSON, or anything that turns into one with `toJSON()`, such as `Composition`. */
export type CompositionSource = LumenComposition | { toJSON(): LumenComposition };
