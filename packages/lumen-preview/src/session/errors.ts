export function describeError(error: unknown): string {
  if (error instanceof Error) {
    return error.stack ?? `${error.name}: ${error.message}`;
  }

  return String(error);
}

export function reportConsoleError(scope: string, error: unknown): void {
  console.error(`[LumenPreviewSession] ${scope}`, error);
}
