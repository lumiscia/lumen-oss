import type { LumenPreviewController } from "lumen-wasm";

export class LumenPreviewContext {
  frame = $state(0);
  totalFrames = $state(0);
  width = $state(0);
  height = $state(0);
  renderMs = $state(0);
  isPlaying = $state(false);
  error = $state<string | null>(null);
  /** The underlying WASM controller — null until lumen-wasm loads. */
  controller = $state<LumenPreviewController | null>(null);

  #ctrl: LumenPreviewController | null = null;
  #seekFn: ((frame: number) => void) | null = null;

  /** @internal — called by LumenCanvas once the WASM controller is ready */
  _attach(ctrl: LumenPreviewController, seekFn: (frame: number) => void): void {
    this.#ctrl = ctrl;
    this.#seekFn = seekFn;
    this.controller = ctrl;
    if (this.isPlaying) ctrl.play();
  }

  /** @internal — called by LumenCanvas on destroy */
  _detach(): void {
    this.#ctrl = null;
    this.#seekFn = null;
    this.controller = null;
  }

  play(): void {
    this.#ctrl?.play();
    this.isPlaying = true;
  }

  pause(): void {
    this.#ctrl?.pause();
    this.isPlaying = false;
  }

  seek(frame: number): void {
    this.#seekFn?.(frame);
    this.frame = frame;
  }
}

export function createLumenPreview(): LumenPreviewContext {
  return new LumenPreviewContext();
}
