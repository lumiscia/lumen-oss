---
"@lumiscia/lumen-sdk": minor
---

Add `verifyWebhook({ body, headers, secret, toleranceSeconds? })`, exported from both the Node and Cloudflare entry points. It verifies a render webhook signed with the [Standard Webhooks](https://www.standardwebhooks.com) scheme (`webhook-id`, `webhook-timestamp`, and `webhook-signature` headers, signed with a `whsec_` secret) using Web Crypto, then returns the typed `RenderWebhookEvent`. If the webhook can't be verified or parsed, it throws a `LumenWebhookError` with a `code`: `missing_headers`, `invalid_secret`, `invalid_signature`, `timestamp_out_of_range`, or `invalid_payload`.
