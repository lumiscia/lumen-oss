import type { LumenPreviewController } from "./index.js";

export interface LumenPreviewState {
  frame: number;
  totalFrames: number;
  width: number;
  height: number;
  isLoaded: boolean;
  fps: number;
  frameDurationMs: number;
  renderMs: number;
  isPlaying: boolean;
  error: string | null;
  controller: LumenPreviewController | null;
}

export type LumenPreviewPatch = Partial<LumenPreviewState>;
export type LumenPreviewListener = () => void;

export interface LumenPreviewTransport {
  pause?: () => void;
  play?: () => void;
  seek?: (frame: number) => void;
}

const INITIAL_STATE: LumenPreviewState = {
  frame: 0,
  totalFrames: 0,
  width: 0,
  height: 0,
  isLoaded: false,
  fps: 0,
  frameDurationMs: 0,
  renderMs: 0,
  isPlaying: false,
  error: null,
  controller: null,
};

export class LumenPreviewContext {
  #state: LumenPreviewState = INITIAL_STATE;
  #listeners = new Set<LumenPreviewListener>();
  #seekFn: ((frame: number) => void) | null = null;
  #transport: LumenPreviewTransport | null = null;

  subscribe = (listener: LumenPreviewListener): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  getSnapshot = (): LumenPreviewState => this.#state;

  update(patch: LumenPreviewPatch): void {
    this.#setState({ ...this.#state, ...patch });
  }

  reset(): void {
    this.#setState({
      ...INITIAL_STATE,
      isPlaying: this.#state.isPlaying,
    });
  }

  attach(
    controller: LumenPreviewController,
    seekFn: (frame: number) => void,
    transport: LumenPreviewTransport | null = null,
  ): void {
    this.#seekFn = seekFn;
    this.#transport = transport;
    this.update({ controller });

    if (this.#state.isPlaying) {
      controller.play();
      this.#transport?.play?.();
    }
  }

  detach(): void {
    this.#seekFn = null;
    this.#transport = null;
    this.#setState({
      ...INITIAL_STATE,
      isPlaying: false,
      error: this.#state.error,
    });
  }

  /** @internal Compatibility alias for framework canvas components. */
  _attach(
    controller: LumenPreviewController,
    seekFn: (frame: number) => void,
    transport: LumenPreviewTransport | null = null,
  ): void {
    this.attach(controller, seekFn, transport);
  }

  /** @internal Compatibility alias for framework canvas components. */
  _detach(): void {
    this.detach();
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

  #emit(): void {
    for (const listener of this.#listeners) {
      listener();
    }
  }

  #setState(nextState: LumenPreviewState): void {
    this.#state = nextState;
    this.#emit();
  }
}

export function createLumenPreview(): LumenPreviewContext {
  return new LumenPreviewContext();
}
