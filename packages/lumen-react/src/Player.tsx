import { useEffect, useRef } from "react";

import type { CSSProperties } from "react";
import { mountPreview, type LumenPreviewSession } from "@lumiscia/lumen-preview";
import type { LumenPreviewSessionInputs, LumenRuntime } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.js";

export interface PlayerProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
  /**
   * The runtime files to load. Defaults to `runtime` from `@lumiscia/lumen-bindings/urls`, or
   * `runtime2d` from `@lumiscia/lumen-bindings/urls-2d` for the `Player` from
   * `@lumiscia/lumen-react/2d`.
   */
  runtime?: LumenRuntime | undefined;
  /**
   * A directory serving every file of `runtime` under its `file` name. Without it, each file is
   * loaded from its `url`.
   */
  runtimeUrl?: string | URL | undefined;
  /** Applied to the element that holds the canvas, which fills it and keeps its aspect ratio. */
  className?: string;
  style?: CSSProperties;
}

/** A `Player` whose runtime defaults to `defaultRuntime`, so an entry point names only the build it uses. */
export function createPlayer(defaultRuntime: LumenRuntime) {
  return function Player(props: PlayerProps) {
    return <PreviewPlayer {...props} defaultRuntime={defaultRuntime} />;
  };
}

function PreviewPlayer({
  defaultRuntime,
  preview,
  runtime = defaultRuntime,
  runtimeUrl,
  className,
  style,
  ...inputs
}: PlayerProps & { defaultRuntime: LumenRuntime }) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const sessionRef = useRef<LumenPreviewSession | null>(null);
  const directory = runtimeUrl === undefined ? undefined : String(runtimeUrl);
  // By content, so a manifest written inline doesn't restart the preview on every render.
  const runtimeKey = JSON.stringify(runtime);

  // The session calls the newest callback through a function whose identity never changes, so a
  // callback written inline doesn't count as a changed input.
  const latestRef = useRef(inputs);
  const callbacksRef = useRef<Partial<Record<CallbackKey, Callback>>>({});
  useEffect(() => {
    latestRef.current = inputs;
  });
  const stableInputs = () => withStableCallbacks(inputs, latestRef, callbacksRef.current);

  useEffect(() => {
    const session = mountPreview(hostRef.current as HTMLDivElement, {
      ...stableInputs(),
      preview,
      runtime,
      runtimeUrl: directory,
    });
    sessionRef.current = session;
    return () => session.dispose();
  }, [preview, runtimeKey, directory]);

  // Reading a composition serializes it, so the session is told only when an input changed.
  const passedRef = useRef<typeof inputs | null>(null);
  useEffect(() => {
    const next = stableInputs();
    const passed = passedRef.current;
    if (passed !== null && sameInputs(passed, next)) {
      return;
    }
    passedRef.current = next;
    sessionRef.current?.setInputs(next);
  });

  return <div ref={hostRef} className={className} style={style} />;
}

type Inputs = LumenPreviewSessionInputs;
type Callback = (...args: never[]) => void;

const callbackKeys = ["onEnded", "onError", "onStats", "onTimeUpdate"] as const;
type CallbackKey = (typeof callbackKeys)[number];

/** Replaces each callback with a function that calls the newest one in `latest`. */
function withStableCallbacks(
  inputs: Inputs,
  latest: { readonly current: Inputs },
  callbacks: Partial<Record<CallbackKey, Callback>>,
): Inputs {
  const stable: Record<string, unknown> = { ...inputs };
  for (const key of callbackKeys) {
    if (typeof inputs[key] === "function") {
      callbacks[key] ??= (...args) =>
        (latest.current[key] as Callback | null | undefined)?.(...args);
      stable[key] = callbacks[key];
    }
  }
  return stable as Inputs;
}

// The composition can be large and callers keep it stable, so only its identity counts.
function sameInputs(left: Inputs, right: Inputs): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]) as Set<keyof Inputs>;
  for (const key of keys) {
    const same = key === "composition" ? Object.is : sameValue;
    if (!same(left[key], right[key])) {
      return false;
    }
  }
  return true;
}

/** Equal by identity, or plain objects or arrays whose entries are identical. */
function sameValue(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) {
    return true;
  }
  if (Array.isArray(left) && Array.isArray(right)) {
    return (
      left.length === right.length && left.every((value, index) => Object.is(value, right[index]))
    );
  }
  if (isPlainObject(left) && isPlainObject(right)) {
    const keys = Object.keys(left);
    return (
      keys.length === Object.keys(right).length &&
      keys.every((key) => Object.hasOwn(right, key) && Object.is(left[key], right[key]))
    );
  }
  return false;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}
