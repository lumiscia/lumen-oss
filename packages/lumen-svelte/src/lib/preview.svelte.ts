/// <reference types="svelte" />

import { LumenPreviewContext as CoreLumenPreviewContext } from "@lumiscia/lumen-preview/preview";

export class LumenPreviewContext extends CoreLumenPreviewContext {
  /** The latest snapshot, for reading in templates and effects. */
  state = $state.raw(this.getSnapshot());

  constructor() {
    super();
    this.subscribe(() => {
      this.state = this.getSnapshot();
    });
  }
}

export function createLumenPreview(): LumenPreviewContext {
  return new LumenPreviewContext();
}
