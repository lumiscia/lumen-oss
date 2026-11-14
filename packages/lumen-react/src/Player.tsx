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

  useEffect(() => {
    const session = mountPreview(hostRef.current as HTMLDivElement, {
      ...inputs,
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
    const passed = passedRef.current;
    if (passed !== null && sameInputs(passed, inputs)) {
      return;
    }
    passedRef.current = inputs;
    sessionRef.current?.setInputs(inputs);
  });

  return <div ref={hostRef} className={className} style={style} />;
}

function sameInputs(left: object, right: object): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
  for (const key of keys) {
    if (!Object.is(left[key as keyof typeof left], right[key as keyof typeof right])) {
      return false;
    }
  }
  return true;
}
