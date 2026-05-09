import type { LumenApiError } from "./types.js";

export function normalizeApiError(error: unknown, response: Response): LumenApiError {
  if (isApiError(error)) {
    return error;
  }

  if (isApiErrorBody(error)) {
    return {
      code: error.code ?? `http_${response.status}`,
      message: error.error,
      ...(error.requestId !== undefined ? { details: { requestId: error.requestId } } : {}),
    };
  }

  if (typeof error === "string") {
    return {
      code: `http_${response.status}`,
      message: error,
    };
  }

  return {
    code: `http_${response.status}`,
    message: response.statusText || "Lumen API request failed.",
  };
}

function isApiError(value: unknown): value is LumenApiError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "message" in value &&
    typeof value.code === "string" &&
    typeof value.message === "string"
  );
}

function isApiErrorBody(value: unknown): value is {
  code?: string;
  error: string;
  requestId?: string;
} {
  return (
    typeof value === "object" &&
    value !== null &&
    "error" in value &&
    typeof value.error === "string" &&
    (!("code" in value) || typeof value.code === "string") &&
    (!("requestId" in value) || typeof value.requestId === "string")
  );
}
