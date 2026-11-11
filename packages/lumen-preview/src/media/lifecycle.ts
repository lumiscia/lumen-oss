import type { MediaFailure, MediaFailureKind, MediaLocation } from "@lumiscia/lumen-types";

import { errorMediaFailure } from "./errors.js";

export type MediaStatus = "pending" | "ready" | "failed";

/** One source the current composition needs, read as one kind. */
export interface MediaDependency<TInput> {
  /** Tells apart the kinds one source is read as. */
  readonly key: string;
  readonly kind: MediaFailureKind;
  /** The source as the composition writes it, or the font family. */
  readonly source: string;
  /** Where the composition first uses it. */
  readonly at?: MediaLocation;
  /** What to load, or a failure for a source that can't be resolved to anything. */
  readonly load: { readonly input: TInput } | { readonly failure: MediaFailure };
}

/** A dependency's state, as the page is told it. */
export interface MediaEntryState {
  readonly kind: MediaFailureKind;
  readonly source: string;
  readonly status: MediaStatus;
  readonly failure: MediaFailure | null;
}

/** How one kind of media is fetched, decoded and handed to the renderer, and taken back. */
export interface MediaLoader<TInput> {
  load(dependency: MediaDependency<TInput>, input: TInput, signal: AbortSignal): Promise<void>;
  /** Frees what `load` registered, also while it is still in flight. */
  release(dependency: MediaDependency<TInput>): void;
  /** Whether two inputs load the same media, so a dependency that keeps it isn't loaded again. */
  same(left: TInput, right: TInput): boolean;
}

type Entry<TInput> = {
  abort: AbortController;
  dependency: MediaDependency<TInput>;
  failure: MediaFailure | null;
  status: MediaStatus;
};

type Waiter<TInput> = {
  includes: (dependency: MediaDependency<TInput>) => boolean;
  resolve: (failure: MediaFailure | null) => void;
};

/**
 * Every dependency of the current composition is `pending` until its kind's loader has it, then
 * `ready` or `failed`. One the composition stops naming is released, and one whose input changed
 * is released and loaded again. A failed one is tried again by the next `sync`.
 */
export class MediaLifecycle<TInput> {
  readonly #entries = new Map<string, Entry<TInput>>();
  readonly #loader: MediaLoader<TInput>;
  readonly #onChange: () => void;
  #waiters: Waiter<TInput>[] = [];

  constructor(loader: MediaLoader<TInput>, onChange: () => void) {
    this.#loader = loader;
    this.#onChange = onChange;
  }

  sync(dependencies: Iterable<MediaDependency<TInput>>): void {
    const desired = new Map<string, MediaDependency<TInput>>();
    for (const dependency of dependencies) {
      desired.set(dependency.key, dependency);
    }
    for (const [key, entry] of this.#entries) {
      if (!desired.has(key)) {
        this.#release(key, entry);
      }
    }
    for (const [key, dependency] of desired) {
      const current = this.#entries.get(key);
      if (current && current.status !== "failed" && this.#keeps(current.dependency, dependency)) {
        current.dependency = dependency;
        continue;
      }
      if (current) {
        this.#release(key, current);
      }
      this.#start(key, dependency);
    }
    this.#changed();
  }

  /** Releases everything. */
  clear(): void {
    this.sync([]);
  }

  entries(): MediaEntryState[] {
    return [...this.#entries.values()].map(({ dependency, failure, status }) => ({
      failure,
      kind: dependency.kind,
      source: dependency.source,
      status,
    }));
  }

  /**
   * Resolves once none of the dependencies `includes` picks is pending, with the first that
   * failed, or null. Later syncs count: it waits for what is current when it resolves.
   */
  settled(
    includes: (dependency: MediaDependency<TInput>) => boolean = () => true,
  ): Promise<MediaFailure | null> {
    return new Promise((resolve) => {
      this.#waiters.push({ includes, resolve });
      this.#resolveWaiters();
    });
  }

  #keeps(current: MediaDependency<TInput>, next: MediaDependency<TInput>): boolean {
    return (
      "input" in current.load &&
      "input" in next.load &&
      this.#loader.same(current.load.input, next.load.input)
    );
  }

  #start(key: string, dependency: MediaDependency<TInput>): void {
    const entry: Entry<TInput> = {
      abort: new AbortController(),
      dependency,
      failure: null,
      status: "pending",
    };
    this.#entries.set(key, entry);
    if ("failure" in dependency.load) {
      entry.status = "failed";
      entry.failure = dependency.load.failure;
      return;
    }
    const { input } = dependency.load;
    void (async () => {
      try {
        await this.#loader.load(dependency, input, entry.abort.signal);
        this.#settle(key, entry, null);
      } catch (error) {
        this.#settle(key, entry, describeFailure(error, dependency));
      }
    })();
  }

  #settle(key: string, entry: Entry<TInput>, failure: MediaFailure | null): void {
    if (this.#entries.get(key) !== entry) {
      return;
    }
    entry.status = failure ? "failed" : "ready";
    entry.failure = failure;
    this.#changed();
  }

  #release(key: string, entry: Entry<TInput>): void {
    this.#entries.delete(key);
    entry.abort.abort();
    if ("input" in entry.dependency.load) {
      this.#loader.release(entry.dependency);
    }
  }

  #changed(): void {
    this.#onChange();
    this.#resolveWaiters();
  }

  #resolveWaiters(): void {
    const waiting: Waiter<TInput>[] = [];
    for (const waiter of this.#waiters) {
      const entries = [...this.#entries.values()].filter(({ dependency }) =>
        waiter.includes(dependency),
      );
      if (entries.some(({ status }) => status === "pending")) {
        waiting.push(waiter);
      } else {
        waiter.resolve(entries.find(({ failure }) => failure)?.failure ?? null);
      }
    }
    this.#waiters = waiting;
  }
}

/** The failure `error` describes for `dependency`, which names where the composition uses it. */
function describeFailure(error: unknown, dependency: MediaDependency<unknown>): MediaFailure {
  const failure: MediaFailure = errorMediaFailure(error) ?? {
    code: "media.decode",
    details: error instanceof Error ? error.message : String(error),
    source: dependency.source,
    wanted: [dependency.kind],
  };
  return {
    ...failure,
    source: dependency.source,
    ...(failure.at === undefined && dependency.at !== undefined && { at: dependency.at }),
  };
}
