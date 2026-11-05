export interface LumenErrorFields {
  readonly cause?: unknown;
  readonly renderId?: string;
  readonly requestId?: string;
  readonly retryAfterMs?: number;
  readonly retryable?: boolean;
  readonly status?: number;
}

/**
 * The error every `Lumen` method throws, apart from an aborted `signal`, which rejects with the
 * abort reason. `code` is the server's error code, or one of `http_<status>`, `network_error`,
 * `invalid_response`, `render_not_ready` and `socket_error` when the failure never reached the
 * server's own error handling.
 */
export class LumenError extends Error {
  readonly code: string;
  /** The HTTP status, when the failure was a response. */
  readonly status?: number;
  readonly requestId?: string;
  /** The render this error is about, when it is about one. */
  readonly renderId?: string;
  /** Whether the server expects retrying the same request to succeed. */
  readonly retryable?: boolean;
  /** How long the server asked to wait before retrying, from a `Retry-After` header. */
  readonly retryAfterMs?: number;

  constructor(code: string, message: string, fields: LumenErrorFields = {}) {
    super(message, fields.cause === undefined ? undefined : { cause: fields.cause });
    this.name = "LumenError";
    this.code = code;
    if (fields.status !== undefined) this.status = fields.status;
    if (fields.requestId !== undefined) this.requestId = fields.requestId;
    if (fields.renderId !== undefined) this.renderId = fields.renderId;
    if (fields.retryable !== undefined) this.retryable = fields.retryable;
    if (fields.retryAfterMs !== undefined) this.retryAfterMs = fields.retryAfterMs;
  }
}

/** Reads a JSON body, or `undefined` when it isn't JSON. An abort rejects with the signal's reason. */
export async function readBody(
  response: Response,
  signal: AbortSignal | undefined,
): Promise<unknown> {
  try {
    return await response.json();
  } catch {
    if (signal?.aborted) {
      throw signal.reason;
    }
    return undefined;
  }
}

/** Reads an error response, whose body is `{ code?, error, requestId? }`. */
export async function errorFromResponse(
  response: Response,
  signal: AbortSignal | undefined,
): Promise<LumenError> {
  const body = await readBody(response, signal);
  const fields = isRecord(body) ? body : {};
  const retryAfterMs = parseRetryAfter(response.headers.get("retry-after"));
  return new LumenError(
    typeof fields.code === "string" ? fields.code : `http_${response.status}`,
    typeof fields.error === "string"
      ? fields.error
      : response.statusText || "Lumen API request failed.",
    {
      status: response.status,
      ...(typeof fields.requestId === "string" ? { requestId: fields.requestId } : {}),
      ...(retryAfterMs !== undefined ? { retryAfterMs } : {}),
    },
  );
}

function parseRetryAfter(header: string | null): number | undefined {
  if (header === null) {
    return undefined;
  }
  const seconds = Number(header);
  if (header.trim() !== "" && Number.isFinite(seconds)) {
    return Math.max(0, seconds * 1000);
  }
  const date = Date.parse(header);
  return Number.isNaN(date) ? undefined : Math.max(0, date - Date.now());
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
