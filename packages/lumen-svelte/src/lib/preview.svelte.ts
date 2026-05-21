/// <reference types="svelte" />

import {
  LumenPreviewContext as CoreLumenPreviewContext,
  createLumenPreview as createCoreLumenPreview,
  type LumenPreviewListener,
  type LumenPreviewPatch,
  type LumenPreviewState,
  type LumenPreviewTransport,
} from "@lumiscia/lumen-preview/preview";
import type { LumenPreviewController } from "@lumiscia/lumen-preview";

export class LumenPreviewContext {
  readonly core: CoreLumenPreviewContext;

  frame = $state(0);
  totalFrames = $state(0);
  width = $state(0);
  height = $state(0);
  isLoaded = $state(false);
  isPlaying = $state(false);
  error = $state<string | null>(null);
  /** The underlying preview controller — null until the canvas mounts. */
  controller = $state<LumenPreviewController | null>(null);

  constructor(core: CoreLumenPreviewContext = createCoreLumenPreview()) {
    this.core = core;
    this.#sync(core.getSnapshot());
    this.core.subscribe(() => {
      this.#sync(this.core.getSnapshot());
    });
  }

  subscribe(listener: LumenPreviewListener): () => void {
    return this.core.subscribe(listener);
  }

  getSnapshot(): LumenPreviewState {
    return this.core.getSnapshot();
  }

  update(patch: LumenPreviewPatch): void {
    this.core.update(patch);
  }

  reset(): void {
    this.core.reset();
  }

  /** @internal — called by LumenCanvas once the WASM controller is ready */
  _attach(
    controller: LumenPreviewController,
    seekFn: (frame: number) => void,
    transport: LumenPreviewTransport | null = null,
  ): void {
    this.core.attach(controller, seekFn, transport);
  }

  /** @internal — called by LumenCanvas on destroy */
  _detach(): void {
    this.core.detach();
  }

  play(): void {
    this.core.play();
  }

  pause(): void {
    this.core.pause();
  }

  seek(frame: number): void {
    this.core.seek(frame);
  }

  #sync(snapshot: LumenPreviewState): void {
    this.frame = snapshot.frame;
    this.totalFrames = snapshot.totalFrames;
    this.width = snapshot.width;
    this.height = snapshot.height;
    this.isLoaded = snapshot.isLoaded;
    this.isPlaying = snapshot.isPlaying;
    this.error = snapshot.error;
    this.controller = snapshot.controller;
  }
}

export function createLumenPreview(): LumenPreviewContext {
  return new LumenPreviewContext();
}
