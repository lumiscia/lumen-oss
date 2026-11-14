import { Webhook } from "standardwebhooks";
import { afterEach, describe, expect, test, vi } from "vitest";

import { LumenWebhookError, verifyWebhook } from "../src/webhooks.js";
import type { LumenWebhookErrorCode, VerifyWebhookOptions } from "../src/webhooks.js";

const secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
const otherSecret = "whsec_dGhpcyBpcyBhIGRpZmZlcmVudCBzZWNyZXQ=";
const webhookId = "msg_p5jXN8AQM9LWM0D4loKWxJek";
const now = 1_761_800_000;
const render = {
  costMicros: 12_500,
  createdAt: "2026-10-30T04:53:20.000Z",
  durationMs: 4_000,
  id: "render-1",
  inputHash: "ab12",
  organizationId: "org-1",
  outputBytes: 1_048_576,
  progress: 1,
  status: "succeeded",
  url: "https://cdn.lumiscia.com/renders/render-1.mp4",
};
const body = JSON.stringify({
  type: "render.succeeded",
  timestamp: "2026-10-30T04:53:24.000Z",
  data: render,
});

function sign(payload: string, options: { secret?: string; timestamp?: number } = {}) {
  return new Webhook(options.secret ?? secret).sign(
    webhookId,
    new Date((options.timestamp ?? now) * 1000),
    payload,
  );
}

function headers(signature: string, timestamp = now) {
  return {
    "webhook-id": webhookId,
    "webhook-signature": signature,
    "webhook-timestamp": String(timestamp),
  };
}

async function expectCode(options: VerifyWebhookOptions, code: LumenWebhookErrorCode) {
  const error = await verifyWebhook(options).catch((cause: unknown) => cause);
  expect(error).toBeInstanceOf(LumenWebhookError);
  expect((error as LumenWebhookError).code).toBe(code);
}

describe("verifyWebhook", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  test("accepts the Standard Webhooks reference signature", async () => {
    vi.useFakeTimers({ now: 1_614_265_330 * 1000 });
    const payload = '{"test": 2432232314}';
    const signature = "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=";
    expect(sign(payload, { timestamp: 1_614_265_330 })).toBe(signature);

    // The signature checks out; the body just isn't a render event.
    await expectCode(
      { body: payload, headers: headers(signature, 1_614_265_330), secret },
      "invalid_payload",
    );
  });

  test("returns the render event for a body signed by the reference library", async () => {
    vi.useFakeTimers({ now: now * 1000 });

    await expect(
      verifyWebhook({ body, headers: headers(sign(body)), secret }),
    ).resolves.toMatchObject({
      type: "render.succeeded",
      timestamp: "2026-10-30T04:53:24.000Z",
      data: { id: "render-1", costMicros: 12_500, url: render.url },
    });
  });

  test("accepts Headers instances, mixed-case records, array values, and byte bodies", async () => {
    vi.useFakeTimers({ now: now * 1000 });
    const signature = sign(body);

    await expect(
      verifyWebhook({ body, headers: new Headers(headers(signature)), secret }),
    ).resolves.toMatchObject({ data: { id: "render-1" } });
    await expect(
      verifyWebhook({
        body: new TextEncoder().encode(body),
        headers: {
          "Webhook-Id": webhookId,
          "WEBHOOK-TIMESTAMP": String(now),
          "webhook-signature": [signature],
        },
        secret,
      }),
    ).resolves.toMatchObject({ data: { id: "render-1" } });
    await expect(
      verifyWebhook({
        body: new TextEncoder().encode(body).buffer,
        headers: headers(signature),
        secret,
      }),
    ).resolves.toMatchObject({ data: { id: "render-1" } });
  });

  test("accepts any matching v1 signature and ignores other versions", async () => {
    vi.useFakeTimers({ now: now * 1000 });
    const current = sign(body);
    const previous = sign(body, { secret: otherSecret });

    await expect(
      verifyWebhook({ body, headers: headers(`v1a,abc ${previous} ${current}`), secret }),
    ).resolves.toMatchObject({ data: { id: "render-1" } });
    await expect(
      verifyWebhook({ body, headers: headers(`${current} ${previous}`), secret: otherSecret }),
    ).resolves.toMatchObject({ data: { id: "render-1" } });
  });

  test("rejects a tampered body or the wrong secret", async () => {
    vi.useFakeTimers({ now: now * 1000 });
    const signature = sign(body);

    await expectCode(
      { body: body.replace("12500", "1"), headers: headers(signature), secret },
      "invalid_signature",
    );
    await expectCode(
      { body, headers: headers(signature), secret: otherSecret },
      "invalid_signature",
    );
    await expectCode(
      { body, headers: headers(signature.replace("v1,", "v2,")), secret },
      "invalid_signature",
    );
  });

  test("rejects expired and future timestamps outside the tolerance", async () => {
    vi.useFakeTimers({ now: now * 1000 });
    const old = now - 301;
    const future = now + 301;

    await expectCode(
      { body, headers: headers(sign(body, { timestamp: old }), old), secret },
      "timestamp_out_of_range",
    );
    await expectCode(
      { body, headers: headers(sign(body, { timestamp: future }), future), secret },
      "timestamp_out_of_range",
    );
    await expect(
      verifyWebhook({
        body,
        headers: headers(sign(body, { timestamp: old }), old),
        secret,
        toleranceSeconds: 600,
      }),
    ).resolves.toMatchObject({ data: { id: "render-1" } });
    await expect(
      verifyWebhook({ body, headers: headers(sign(body)), secret, toleranceSeconds: Number.NaN }),
    ).rejects.toThrow(RangeError);
  });

  test("rejects missing headers, a bad secret, and a body that isn't a render event", async () => {
    vi.useFakeTimers({ now: now * 1000 });
    const { "webhook-id": _, ...withoutId } = headers(sign(body));

    await expectCode({ body, headers: withoutId, secret }, "missing_headers");
    await expectCode({ body, headers: headers(sign(body)), secret: "whsec_!!" }, "invalid_secret");

    const flat = JSON.stringify({ event: "render.failed", id: "render-1", status: "failed" });
    await expectCode({ body: flat, headers: headers(sign(flat)), secret }, "invalid_payload");
    // The event says failed and its render says succeeded.
    const mismatched = JSON.stringify({ type: "render.failed", timestamp: "t", data: render });
    await expectCode(
      { body: mismatched, headers: headers(sign(mismatched)), secret },
      "invalid_payload",
    );
  });

  test("returns a failed render with why it failed", async () => {
    vi.useFakeTimers({ now: now * 1000 });
    const error = { code: "media.not_found", message: "hero.png was not found", retryable: false };
    const failed = JSON.stringify({
      type: "render.failed",
      timestamp: "2026-10-30T04:53:24.000Z",
      data: { ...render, status: "failed", progress: 0.2, url: undefined, error },
    });

    await expect(
      verifyWebhook({ body: failed, headers: headers(sign(failed)), secret }),
    ).resolves.toMatchObject({ type: "render.failed", data: { status: "failed", error } });
  });
});
