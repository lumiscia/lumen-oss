/** An animation clip of a binary glTF (`.glb`) model. */
export interface LumenModelAnimation {
  /** The clip's position in the file. A `model_3d`'s `animation` selects a clip by this, as text. */
  readonly index: number;
  /** What a `model_3d`'s `animation` selects the clip by. Absent when the file doesn't name it. */
  readonly name?: string;
  /** Seconds from the clip's start to its last key. */
  readonly duration: number;
}

/** A material of a binary glTF (`.glb`) model. */
export interface LumenModelMaterial {
  readonly index: number;
  /**
   * What a `model_3d`'s `texture_material` selects the material by. Absent when the file doesn't
   * name it, which leaves the material unselectable.
   */
  readonly name?: string;
  /** Whether a texture multiplies the material's base colour, which a `texture` input replaces. */
  readonly hasBaseColorTexture: boolean;
}

/** What a binary glTF (`.glb`) model offers a `model_3d` to choose from. */
export interface LumenModelInfo {
  readonly animations: readonly LumenModelAnimation[];
  readonly materials: readonly LumenModelMaterial[];
}
