# Contract: JSX Compiler Inputs and Diagnostics

## Purpose

Define what JSX constructs are accepted for rewrite scope and how compile failures are surfaced.

## Input contract

1. Supported primitives MUST be explicitly listed and mapped to canonical JSON semantics.
2. Supported components MUST declare composition constraints (allowed children, required props, structural invariants).
3. Unknown primitive/component names MUST fail compile.
4. Invalid nesting/structure MUST fail compile.
5. Compile output for equivalent semantics MUST be deterministic.

## Output contract

- On success: canonical project JSON fragment(s) compatible with renderer contract.
- On failure: typed diagnostic envelope.

## Diagnostic envelope

Required fields:
- `code`: stable machine-readable error code.
- `message`: human-readable explanation.
- `path`: location context for failing construct.

Optional fields:
- `hint`: remediation suggestion.
- `details`: supplemental metadata for tooling/debugging.

## Required failure classes

- `unsupported_primitive`
- `unsupported_component`
- `invalid_child_node`
- `missing_required_field`
- `invalid_project_root`
- `canonical_contract_validation_failed`

## Acceptance expectations

- All invalid fixture classes MUST produce diagnostics with both failing construct and reason.
- Diagnostics MUST be sanitized when surfaced beyond internal boundaries.
- Failed compilation MUST block renderer submission.
