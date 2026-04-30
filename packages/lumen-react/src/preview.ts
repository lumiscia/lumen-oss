import { useSyncExternalStore } from "react";

import type { LumenPreviewController } from "lumen-wasm";

export interface LumenPreviewState {
  frame: number;
  totalFrames: number;
  width: number;
  height: number;
  renderMs: number;
  isPlaying: boolean;
  error: string | null;
  controller: LumenPreviewController | null;
}

type Listener = () => void;
type PreviewTransport = {
  pause?: () => void;
  play?: () => void;
  seek?: (frame: number) => void;
};

const INITIAL_STATE: LumenPreviewState = {
  frame: 0,
  totalFrames: 0,
  width: 0,
  height: 0,
  renderMs: 0,
  isPlaying: false,
  error: null,
  controller: null,
};

export class LumenPreviewContext {
  #state: LumenPreviewState = INITIAL_STATE;
  #listeners = new Set<Listener>();
  #seekFn: ((frame: number) => void) | null = null;
  #transport: PreviewTransport | null = null;

  subscribe = (listener: Listener): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  getSnapshot = (): LumenPreviewState => this.#state;

  #emit(): void {
    for (const listener of this.#listeners) {
      listener();
    }
  }

  #setState(nextState: LumenPreviewState): void {
    this.#state = nextState;
    this.#emit();
  }

  update(patch: Partial<LumenPreviewState>): void {
    this.#setState({ ...this.#state, ...patch });
  }

  reset(): void {
    this.#setState({
      ...INITIAL_STATE,
      isPlaying: this.#state.isPlaying,
    });
  }

  _attach(
    ctrl: LumenPreviewController,
    seekFn: (frame: number) => void,
    transport: PreviewTransport | null = null,
  ): void {
    this.#seekFn = seekFn;
    this.#transport = transport;
    this.update({ controller: ctrl });
    if (this.#state.isPlaying) {
      ctrl.play();
      this.#transport?.play?.();
    }
  }

  _detach(): void {
    this.#seekFn = null;
    this.#transport = null;
    this.#setState({
      ...INITIAL_STATE,
      isPlaying: false,
      error: this.#state.error,
    });
  }

  play(): void {
    this.#state.controller?.play();
    this.#transport?.play?.();
    this.update({ isPlaying: true });
  }

  pause(): void {
    this.#state.controller?.pause();
    this.#transport?.pause?.();
    this.update({ isPlaying: false });
  }

  seek(frame: number): void {
    this.#seekFn?.(frame);
    this.#transport?.seek?.(frame);
    this.update({ frame });
  }
}

export function createLumenPreview(): LumenPreviewContext {
  return new LumenPreviewContext();
}

export function useLumenPreview(preview: LumenPreviewContext): LumenPreviewState {
  return useSyncExternalStore(preview.subscribe, preview.getSnapshot, preview.getSnapshot);
}
