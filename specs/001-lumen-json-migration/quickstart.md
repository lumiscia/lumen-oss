# Quickstart: Lumen JSON Migration + JSX Rewrite Scope

## Goal

Implement and verify full cutover to canonical project JSON across editor, lumen, lumen-jsx, and templates, with explicit JSX primitive/component/compiler contract coverage.

## 1) Align contract surface

1. Confirm delegate request contract: `input_schema_revision=chat_story_v1` and payload deserializes as `JsonProject`.
2. Remove legacy-versioned branching in touched serialization/validation paths.
3. Align producer payloads to delegate keys (`canvas`, `timeline`, optional `sources`/`layers`) and accepted compatibility behavior (`duration_frames` alias, default `kind=clip`).
4. Enforce producer-side boundary validation so mixed/legacy payloads fail before delegate submission.

## 2) Update producer workflows

1. Update editor export path to emit canonical contract shape only.
2. Update `packages/lumen` compile/delegate entry validation to reject non-delegate payloads before conversion.
3. Update `packages/lumen-jsx` compile path for required primitive/component support and deterministic output behavior.
4. Update templates workflow to produce canonical project JSON and preserve typed contract diagnostics.

## 3) Define JSX rewrite contract coverage

1. Enumerate required primitives and components for rewrite completion.
2. Enforce composition constraints and unsupported-node diagnostics.
3. Ensure deterministic canonical output for equivalent JSX semantics.

## 4) Verify compatibility with Rust consumer

1. Validate canonical payload fixtures against Rust delegate behavior in `crates/lumen/src/json` (schema revision gate, parse, conversion statuses).
2. Ensure invalid legacy/mixed fixtures are rejected by producer-side validation and mapped to delegate-compatible failure handling (`ValidationError` or `ConversionError`).

## 5) Run targeted verification

Run only the tests changed for this feature:

```bash
pnpm --filter @lumiscia/lumen test
pnpm --filter @lumiscia/lumen-jsx test
pnpm --filter @lumiscia/templates test
pnpm --filter @lumiscia/editor test
cargo test -p lumen --features json
cargo test -p lumen
```

## 6) Completion checklist

- Canonical payload fields and delegate request envelope are aligned across all four producer workflows.
- Legacy versioned format is no longer emitted or accepted in touched producer boundaries.
- JSX primitive/component scope and diagnostics are covered by tests.
- Determinism checks for equivalent JSX inputs pass.
- Rust delegate tests pass for both `--features json` and non-json capability-disabled behavior.

## 7) Success criteria evidence mapping

- **SC-001** (canonical payload validation across workflows):
  - `packages/lumen/src/compile.parity.test.ts` verifies canonical delegate request and payload validation.
  - `packages/templates/src/presets/chat-story-v1/chat-story.contract.test.ts` verifies canonical payload emission from presets.
  - `packages/lumen-jsx/src/jsx.contract.test.ts` verifies canonical output from supported JSX inventories.
- **SC-002** (legacy format removal in touched boundaries):
  - `packages/lumen/src/compile.parity.test.ts` includes legacy/mixed marker rejection assertions.
  - `packages/templates/src/presets/chat-story-v1/chat-story.contract.test.ts` includes mixed legacy payload rejection assertions.
  - `apps/editor/src/lib/lumen-client.ts` rejects mixed/legacy payload markers before submission.
- **SC-003** (JSX primitive/component completion):
  - `packages/lumen-jsx/src/jsx.contract.test.ts` table-driven supported primitive and component inventory coverage.
- **SC-004** (equivalent render-intent parity):
  - `packages/lumen/src/compile.parity.test.ts` deterministic canonical projection parity for equivalent payload semantics.
- **SC-005** (diagnostics for invalid JSX):
  - `packages/lumen-jsx/src/jsx.contract.test.ts` asserts `unsupported_primitive`, `unsupported_component`, and `canonical_contract_validation_failed` envelopes.
- **SC-006** (export flows require no manual payload edits):
  - `packages/lumen/src/compile.parity.test.ts` validates delegate envelope builder emits renderer-ready payload shape directly.
  - `packages/templates/src/index.ts` routes preset compilation through canonical payload validation before compatibility projection.

## 8) Verification run log

All targeted verification commands completed successfully in this implementation cycle:

```bash
pnpm --filter @lumiscia/lumen test                         # PASS
pnpm --filter @lumiscia/lumen-jsx test                     # PASS
pnpm --filter @lumiscia/templates test                     # PASS
pnpm --filter @lumiscia/editor test                        # PASS
cargo test -p lumen --features json                        # PASS
cargo test -p lumen                                        # PASS
pnpm --filter @lumiscia/editor-app typecheck              # PASS (editor app integration safety check)
pnpm dlx vitest run apps/editor/tests/project-schema.contract.test.ts apps/editor/tests/lumen-client.test.ts # PASS
```
