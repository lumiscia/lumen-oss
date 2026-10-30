const secretPrefix = "whsec_";
const encoder = new TextEncoder();

export type LumenWebhookErrorCode =
  | "invalid_payload"
  | "invalid_secret"
  | "invalid_signature"
  | "missing_headers"
  | "timestamp_out_of_range";

/** The error `verifyWebhook` throws when a webhook can't be trusted or parsed. */
export class LumenWebhookError extends Error {
  readonly code: LumenWebhookErrorCode;

  constructor(code: LumenWebhookErrorCode, message: string) {
    super(message);
    this.name = "LumenWebhookError";
    this.code = code;
  }
}

/** Header values as a `Headers` instance or a plain record such as Node's `IncomingHttpHeaders`. */
export type WebhookHeaders =
  | Headers
  | Readonly<Record<string, string | readonly string[] | undefined>>;

export interface VerifyWebhookOptions {
  /** The raw request body, exactly as received. Parsing and re-serializing JSON breaks the signature. */
  readonly body: string | ArrayBuffer | Uint8Array;
  readonly headers: WebhookHeaders;
  /** The workspace's webhook signing secret, `whsec_` followed by base64. */
  readonly secret: string;
  /** How far `webhook-timestamp` may be from the current time, in seconds. Defaults to 300. */
  readonly toleranceSeconds?: number;
}

interface RenderWebhookEventFields {
  /** Render id. */
  readonly id: string;
  readonly organizationId: string;
  readonly createdAt: string;
  /** Render cost in millionths of a US dollar. */
  readonly costMicros: number;
  readonly durationMs?: number;
  readonly outputBytes?: number;
  readonly resolution?: string;
  readonly artifactUrl?: string;
  readonly error?: string;
}

export type RenderWebhookEvent =
  | (RenderWebhookEventFields & {
      readonly event: "render.succeeded";
      readonly status: "succeeded";
    })
  | (RenderWebhookEventFields & {
      readonly event: "render.failed";
      readonly status: "failed";
    });

/**
 * Verifies a Lumiscia render webhook signed with the Standard Webhooks scheme and returns its
 * event. Throws a `LumenWebhookError` if the headers are missing, the timestamp is outside the
 * tolerance, no `v1` signature matches, or the body isn't a render event.
 */
export async function verifyWebhook(options: VerifyWebhookOptions): Promise<RenderWebhookEvent> {
  const id = header(options.headers, "webhook-id");
  const timestamp = header(options.headers, "webhook-timestamp");
  const signature = header(options.headers, "webhook-signature");
  if (!id || !timestamp || !signature) {
    throw new LumenWebhookError(
      "missing_headers",
      "Missing webhook-id, webhook-timestamp, or webhook-signature header.",
    );
  }
  if (!/^\d+$/.test(timestamp)) {
    throw new LumenWebhookError("missing_headers", "webhook-timestamp is not a unix timestamp.");
  }

  const toleranceSeconds = options.toleranceSeconds ?? 300;
  if (!Number.isFinite(toleranceSeconds) || toleranceSeconds < 0) {
    throw new RangeError("toleranceSeconds must be a finite, non-negative number.");
  }
  const nowSeconds = Math.floor(Date.now() / 1000);
  if (Math.abs(nowSeconds - Number(timestamp)) > toleranceSeconds) {
    throw new LumenWebhookError(
      "timestamp_out_of_range",
      "webhook-timestamp is outside the allowed tolerance.",
    );
  }

  // `signed` is a copy, so the bytes parsed below are the bytes that were verified even if the
  // caller's buffer changes while this awaits.
  const body = bodyBytes(options.body);
  const prefix = encoder.encode(`${id}.${timestamp}.`);
  const signed = new Uint8Array(prefix.length + body.length);
  signed.set(prefix);
  signed.set(body, prefix.length);
  const key = await crypto.subtle.importKey(
    "raw",
    secretKey(options.secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["verify"],
  );

  // `subtle.verify` compares in constant time. Any matching `v1` entry is accepted so a sender can
  // sign with more than one secret; other versions are ignored.
  let verified = false;
  for (const entry of signature.split(" ")) {
    const [version, value] = entry.split(",", 2);
    const candidate = version === "v1" && value ? decodeBase64(value) : undefined;
    if (candidate && (await crypto.subtle.verify("HMAC", key, candidate, signed))) {
      verified = true;
      break;
    }
  }
  if (!verified) {
    throw new LumenWebhookError("invalid_signature", "No webhook signature matched.");
  }

  let payload: unknown;
  try {
    payload = JSON.parse(new TextDecoder().decode(signed.subarray(prefix.length)));
  } catch {
    throw new LumenWebhookError("invalid_payload", "Webhook body is not valid JSON.");
  }
  if (!isRenderWebhookEvent(payload)) {
    throw new LumenWebhookError("invalid_payload", "Webhook body is not a render event.");
  }
  return payload;
}

function header(headers: WebhookHeaders, name: string): string | undefined {
  if (typeof headers.get === "function") {
    return (headers as Headers).get(name) ?? undefined;
  }
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() === name) {
      return typeof value === "string" ? value : value?.join(" ");
    }
  }
  return undefined;
}

function bodyBytes(body: VerifyWebhookOptions["body"]): Uint8Array {
  if (typeof body === "string") {
    return encoder.encode(body);
  }
  return body instanceof Uint8Array ? body : new Uint8Array(body);
}

function secretKey(secret: string): Uint8Array<ArrayBuffer> {
  const key = decodeBase64(
    secret.startsWith(secretPrefix) ? secret.slice(secretPrefix.length) : secret,
  );
  if (!key || key.length === 0) {
    throw new LumenWebhookError(
      "invalid_secret",
      "Webhook secret must be whsec_ followed by base64.",
    );
  }
  return key;
}

function decodeBase64(value: string): Uint8Array<ArrayBuffer> | undefined {
  try {
    return Uint8Array.from(atob(value), (char) => char.charCodeAt(0));
  } catch {
    return undefined;
  }
}

function isRenderWebhookEvent(value: unknown): value is RenderWebhookEvent {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const event = value as Record<string, unknown>;
  const statusMatches =
    (event.event === "render.succeeded" && event.status === "succeeded") ||
    (event.event === "render.failed" && event.status === "failed");
  return (
    statusMatches &&
    typeof event.id === "string" &&
    typeof event.organizationId === "string" &&
    typeof event.createdAt === "string" &&
    typeof event.costMicros === "number" &&
    optional(event.durationMs, "number") &&
    optional(event.outputBytes, "number") &&
    optional(event.resolution, "string") &&
    optional(event.artifactUrl, "string") &&
    optional(event.error, "string")
  );
}

function optional(value: unknown, type: "number" | "string"): boolean {
  return value === undefined || typeof value === type;
}
