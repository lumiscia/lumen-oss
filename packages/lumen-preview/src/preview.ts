import type { Diagnostic, MediaFailure } from "@lumiscia/lumen-types";

export type LumenPreviewErrorCode =
  /** A source named in the composition's audio couldn't be decoded. The video keeps playing. */
  | "audio_failed"
  /** An option has a value it can't use, such as a `rate` outside the supported range. */
  | "invalid_option"
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
   * What is wrong with the composition, when it didn't load because the engine rejected it; for a
   * `render_failed` error, the engine's code for the failure (such as `render.gpu`, `render.limit`
   * or `render.node_evaluation`) as one error diagnostic with an empty `path`. Empty for every
   * other failure, and for a render that failed on a media source, which `media` says.
   */
  diagnostics: readonly Diagnostic[];
  /**
   * The media source that couldn't be used, when that is why the composition didn't load or a
   * frame didn't render: which source, what was wanted, why it failed (`media.not_found`,
   * `media.wrong_kind`, `media.unsupported`, `media.too_large` or `media.decode`) and where the
   * composition uses it. Null for every other failure.
   */
  media: MediaFailure | null;
}

/**
 * How many of the sources and fonts the current composition needs are still loading, have loaded
 * and failed, every kind counted alike.
 */
export interface LumenPreviewMediaState {
  pending: number;
  ready: number;
  failed: number;
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
  media: LumenPreviewMediaState;
}

export type LumenPreviewPatch = Partial<LumenPreviewState>;
export type LumenPreviewListener = () => void;

export interface LumenPreviewTransport {
  pause(): void;
  play(): void;
  seek(frame: number): Promise<boolean>;
  whenReady(): Promise<LumenPreviewError | null>;
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
  media: { failed: 0, pending: 0, ready: 0 },
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

  /**
   * Resolves with null once the composition the attached session shows has loaded with everything
   * it needs, or with the failure `onError` was called with. A change of composition before then
   * waits for the new one. Resolves with null at once when no session is attached.
   */
  whenReady(): Promise<LumenPreviewError | null> {
    return this.#transport?.whenReady() ?? Promise.resolve(null);
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
