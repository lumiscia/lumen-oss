export function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function reportConsoleError(scope: string, error: unknown): void {
  console.error(`[LumenPreviewSession] ${scope}`, error);
}
