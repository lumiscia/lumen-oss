# Phase 0 Research: Lumen JSON Format Migration

## Contract Alignment Across TypeScript Producers and Rust Consumer

- Decision: Use a fixture-driven executable contract where the canonical fixture corpus is consumed by both TypeScript and Rust JSON-feature tests, with delegate request schema revision `chat_story_v1` as the compatibility gate and payload shape aligned to `JsonProject` (`canvas`, `timeline`, optional `sources`/`layers`).
- Rationale:
  - Current contract definitions are duplicated between exported TypeScript contract objects and fixture files, which allows silent drift.
  - Rust JSON behavior is enforced by deserialization and conversion paths but is not currently validated against the same fixture corpus used by TypeScript producers.
  - This rewrite intentionally drops legacy versioned compatibility, so one explicit schema revision gate is clearer than version-range branching.
  - Delegate compatibility includes accepted alias/default behavior (`timeline.duration_frames` alias, default empty `sources`/`layers`, default `kind=clip`), and fixtures must assert these explicitly.
- Alternatives considered:
  - Rust-first generated schema/types for TS consumers: stronger single source of truth but too much tooling overhead for this migration phase.
  - Manual sync through code review only: lowest immediate effort but insufficient cross-language drift protection.

## JSX Compiler Determinism and Diagnostics

- Decision: Standardize JSX compiler failures on a typed diagnostic envelope (`code`, `path`, `message`, `hint`, optional details) and enforce deterministic canonical output for equivalent inputs through stable traversal and canonicalized object projections in parity tests.
- Rationale:
  - Existing repo patterns already favor typed contract errors in templates and JSX contract tests.
  - Deterministic outputs are already partially validated in compile parity tests and should be extended to structurally equivalent JSX inputs.
  - A typed diagnostic surface enables predictable failure assertions and better migration ergonomics for unsupported primitives/components.
- Alternatives considered:
  - Plain string errors: easy to emit but brittle for tooling and weak for boundary-level assertions.
  - Recursive full sorting of arrays and objects before compare: can hide semantic ordering bugs in timeline/layer arrays.

## Migration Cutover and Boundary Validation Strategy

- Decision: Perform hard cutover to delegate-compatible canonical JSON at producer ingress boundaries, reject legacy or mixed payloads fail-closed with sanitized `invalid_payload` responses, and block compile/render submission when canonical validation fails.
- Rationale:
  - Rust delegate deserialization is permissive for unknown fields by default, so producers must enforce stricter boundary validation and must not rely on unknown-field tolerance for compatibility.
  - Boundary-first strict validation prevents malformed data from reaching compile/queue stages.
  - This aligns with constitutional requirements for external boundary validation and sanitized failure surfaces.
- Alternatives considered:
  - Transitional dual-accept parser with normalization: rejected because it violates full-cutover intent and extends ambiguity.
  - Compile-time-only validation with permissive ingress: rejected because invalid payloads enter the system too early and produce harder-to-debug failures.

## External References Used

- Serde container and field attribute guidance (unknown-field defaults and alias behavior)
- RFC 8785 JSON Canonicalization guidance for deterministic object representation
- TypeScript Compiler API and Babel parser diagnostic patterns
- Zod strict object guidance for producer boundary hardening before delegate submission
