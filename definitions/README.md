# Lumen Definitions

Generated definitions of Lumen's formats: composition documents, the values the engine returns, and the HTTP API.

This repository contains machine-readable artifacts generated from the canonical Lumen source:

- `composition.schema.json` - JSON Schema for Lumen composition documents, including `x-lumen-*` annotations for node metadata, property metadata, defaults, ports, and enum labels
- `runtime.schema.json` - JSON Schema for what the engine returns: diagnostics, media failures, the media inventory and model info
- `openapi.json` - the OpenAPI 3.1 document of the HTTP API: every route of `lumen-server`, its request and response types, the error codes each status carries, the render socket's events and the render webhook. It refers to the two schemas above by relative `$ref`, so keep the three files together

These files are generated automatically and should not be edited by hand.

## Usage

Reference the schemas directly from this repository, or vendor them into tooling that needs to validate Lumen compositions.

```json
{
  "$schema": "https://raw.githubusercontent.com/lumiscia/lumen-oss/main/definitions/composition.schema.json"
}
```

## Updates

Regenerate these files from the repository root with:

```bash
just generate-definitions
```

## Hosts of the API

`lumen-server` is one host of the API in `openapi.json` and the hosted Lumen API is another. `lumen-server` serves exactly the document's routes, and its tests check every response against it. Any host can be checked with the SDK's suite, which sends real requests and validates each answer against the document:

```bash
LUMEN_API_URL=https://host.example/v1 LUMEN_API_TOKEN=... LUMEN_API_PROFILE=profile.json pnpm test:conformance
```

A host that differs on purpose says so in the profile, and anything else it does differently fails the run:

```json
{
  "unsupported": ["cancelRender"],
  "extraValues": { "MediaStatus": ["uploaded"] },
  "extraStatuses": [429],
  "extraCodes": ["rate_limited"]
}
```

`unsupported` lists operations the host doesn't have, `extraValues` adds values to a schema the document marks `x-lumen-open`, `extraStatuses` are error statuses it answers on any route, and `extraCodes` are error codes of its own. `GET /health` reports the `info.version` of the document a host implements.

CI runs `just verify-definitions` to compare the committed files against freshly generated output. If a node schema changes, update the Rust source first, regenerate `definitions/`, and commit both changes together.
