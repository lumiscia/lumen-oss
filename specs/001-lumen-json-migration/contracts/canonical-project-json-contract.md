# Contract: Canonical Project JSON (Delegate-Aligned)

## Purpose

Define the JSON payload and request envelope that TypeScript producers emit so `crates/lumen/src/json` accepts and converts them under the `json` feature.

## Producer/Consumer

- Producers: editor workflow, core project library workflow, JSX compiler workflow, template workflow.
- Consumer: `convert_json_delegate` in `crates/lumen/src/json/mod.rs` and `JsonProject` in `crates/lumen/src/json/enabled.rs`.

## Delegate-aligned request contract

1. Request envelope MUST send `input_schema_revision: "chat_story_v1"`.
2. Request envelope MUST send `input_payload` as JSON matching `JsonProject`.
3. Schema revision is part of delegate request metadata, not a field inside payload JSON.

## Delegate-aligned payload contract (`JsonProject`)

1. Payload root MUST include `canvas` and `timeline`.
2. Payload MAY include `sources` and `layers`; delegate defaults both to empty arrays when omitted.
3. `timeline` MUST include `fps` and `total_frames` (delegate also accepts alias `duration_frames`).
4. Layer items MUST use kind `clip` (or omit kind and rely on delegate default `clip`).
5. Clip content types supported by delegate are `shape`, `text`, `image`, and `video`.
6. Producer outputs SHOULD emit only delegate-defined fields; delegate currently tolerates unknown JSON fields, so producers MUST NOT rely on unknown-field acceptance for compatibility.

## Validation outcomes (delegate statuses)

- Success -> `status=Success`, `project_bundle` present, no errors.
- Unsupported schema revision or invalid JSON -> `status=ValidationError`, `code=validation_error`.
- JSON parses but cannot convert semantically -> `status=ConversionError`, `code=conversion_error`.
- Feature disabled build -> `status=CapabilityDisabled`, `code=capability_disabled`.

## Contract test matrix

- Valid minimal delegate payload (`canvas`, `timeline`, optional empty lists).
- Valid payload using `timeline.duration_frames` alias compatibility.
- Invalid schema revision envelope (`legacy_v0` etc.).
- Invalid JSON payload syntax.
- Conversion-error payload (e.g., image source missing required `path`/`url`/`filter`).
- Determinism pair fixtures (equivalent inputs produce equivalent canonical projection).