import type { ChildProcess } from "node:child_process";
import process from "node:process";

const SIGNALS = ["SIGINT", "SIGTERM", "SIGHUP"] as const;
type Signal = (typeof SIGNALS)[number];

const children = new Set<ChildProcess>();
/** A signal the caller has no handler for, which ends this process once the children have exited. */
let pending: Signal | undefined;

const listeners = SIGNALS.map((signal) => [signal, () => forward(signal)] as const);

function forward(signal: Signal): void {
  // This listener runs first, so a one-shot handler of the caller's is still counted.
  if (process.listenerCount(signal) === 1) {
    pending ??= signal;
  }
  for (const child of children) {
    child.kill(signal);
  }
}

/**
 * Passes a SIGINT, SIGTERM or SIGHUP sent to this process on to `child` until the returned
 * function is called, which is when the child has exited. A signal sent to this process alone, as
 * a supervisor does, would otherwise leave the child running. Listening for a signal stops it
 * ending this process, so when the caller has no handler of its own, the signal is sent again
 * once every child has exited.
 */
export function forwardSignals(child: ChildProcess): () => void {
  if (children.size === 0) {
    for (const [signal, listener] of listeners) {
      process.prependListener(signal, listener);
    }
  }
  children.add(child);
  if (pending !== undefined) {
    child.kill(pending);
  }
  return () => {
    if (!children.delete(child) || children.size > 0) {
      return;
    }
    for (const [signal, listener] of listeners) {
      process.off(signal, listener);
    }
    if (pending !== undefined) {
      const signal = pending;
      pending = undefined;
      process.kill(process.pid, signal);
    }
  };
}
