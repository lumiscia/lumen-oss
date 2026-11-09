import type { LumenDiagnostic } from "@lumiscia/lumen-types";

export type LumenPreviewErrorCode =
  /** A source named in the composition's audio couldn't be decoded. The video keeps playing. */
  | "audio_failed"
  /** The composition, or a media source or font it needs, failed to load. */
  | "load_failed"
  | "render_failed"
  /** This browser or page has no WebGPU, which the preview needs. */
  | "webgpu_unavailable"
  /** The worker or its wasm couldn't start, or the worker later crashed. */
  | "worker_failed";

/** What `onError` receives when the preview fails. */
export interface LumenPreviewError {
  code: LumenPreviewErrorCode;
  message: string;
  /**
   * What is wrong with the composition, when it didn't load because the engine rejected it. Empty
   * for every other failure.
   */
  diagnostics: readonly LumenDiagnostic[];
}

export interface LumenPreviewState {
  frame: number;
  totalFrames: number;
  width: number;
  height: number;
  isLoaded: boolean;
  isPlaying: boolean;
  error: string | null;
  errorCode: LumenPreviewErrorCode | null;
}

export type LumenPreviewPatch = Partial<LumenPreviewState>;
export type LumenPreviewListener = () => void;

export interface LumenPreviewTransport {
  pause(): void;
  play(): void;
  seek(frame: number): Promise<boolean>;
}

const INITIAL_STATE: LumenPreviewState = {
  frame: 0,
  totalFrames: 0,
  width: 0,
  height: 0,
  isLoaded: false,
  isPlaying: false,
  error: null,
  errorCode: null,
};

export class LumenPreviewContext {
  #state: LumenPreviewState = INITIAL_STATE;
  #listeners = new Set<LumenPreviewListener>();
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

  attach(transport: LumenPreviewTransport): void {
    this.#transport = transport;
  }

  detach(): void {
    this.#transport = null;
    this.#setState({
      ...INITIAL_STATE,
      error: this.#state.error,
      errorCode: this.#state.errorCode,
    });
  }

  /** Does nothing until a composition has loaded. */
  play(): void {
    this.#transport?.play();
  }

  pause(): void {
    this.#transport?.pause();
  }

  /**
   * Resolves once the frame has been submitted for display, or a newer seek's frame has, and to
   * false if a load, failure or dispose got in first. It never rejects; failures reach the state.
   */
  seek(frame: number): Promise<boolean> {
    if (this.#transport) {
      return this.#transport.seek(frame);
    }
    this.update({ frame });
    return Promise.resolve(false);
  }

  #setState(nextState: LumenPreviewState): void {
    this.#state = nextState;
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

export function createLumenPreview(): LumenPreviewContext {
  return new LumenPreviewContext();
}
