# Data Model: Lumen JSON Format Migration

## 1) JsonDelegateRequest

Represents the delegate request envelope consumed by `convert_json_delegate`.

### Core fields
- `input_schema_revision`: schema revision gate (must be `chat_story_v1`).
- `input_payload`: JSON string deserialized as `JsonProject`.
- `caller_context`: caller-supplied context for observability.

### Validation rules
- Unsupported schema revision fails with validation error before payload parsing.
- Invalid JSON syntax fails with validation error.

### Relationships
- Carries `ProjectJsonPayload` to Rust delegate.
- Produces `JsonDelegateResult` with status/issues.

---

## 2) ProjectJsonPayload

Represents the delegate payload JSON (`JsonProject`) produced by in-scope workflows.

### Core fields
- `canvas`: required object (`width`, `height`, optional `background` defaulted by delegate).
- `timeline`: required object (`fps`, `total_frames`; alias `duration_frames` accepted).
- `sources`: optional array of source descriptors (defaults to empty).
- `layers`: optional array of layer descriptors (defaults to empty).

### Validation rules
- Payload MUST include fields required by delegate conversion (`canvas`, `timeline`).
- Layer item `kind` MUST be `clip` when present; omitted values default to `clip`.
- Producers SHOULD emit only delegate-defined fields; unknown fields may deserialize but are not compatibility-safe contract output.

### Relationships
- Produced from `AuthoringProjectInput` via serialization/compilation paths.
- Consumed by Rust delegate JSON feature path.
- Validated using `CanonicalContractDefinition`.

---

## 3) AuthoringProjectInput

Represents source project content from editor, JSX, and template workflows before canonical serialization.

### Variants
- Editor-authored project input.
- JSX-authored project tree.
- Template preset input payload.

### Validation rules
- MUST be validated at workflow boundaries before conversion.
- Unsupported primitive/component structures MUST fail with compile diagnostics.
- Mixed legacy+canonical structures MUST be rejected by producer boundary validation.

### Relationships
- Transformed into `ProjectJsonPayload`.
- Invalid authoring input produces `CompilationDiagnostic` and blocks submission.

---

## 4) JsxPrimitiveDefinition

Atomic JSX authoring construct mapped to canonical delegate payload nodes.

### Core fields
- `name`: primitive identifier.
- `requiredProps`: required property set.
- `optionalProps`: optional property set.
- `allowedChildren`: allowed child primitive/component categories.
- `mappingRule`: mapping to delegate-compatible payload node(s).

### Validation rules
- Every supported primitive MUST have explicit mapping and constraints.
- Unknown primitive names MUST fail with `unsupported_primitive` diagnostics.
- Invalid child composition MUST fail with deterministic diagnostics.

### Relationships
- Composed within `JsxComponentContract`.
- Compiled into `ProjectJsonPayload` nodes through JSX compiler.

---

## 5) JsxComponentContract

Reusable JSX component contract composed from primitives and/or nested component contracts.

### Core fields
- `componentName`: exported component identifier.
- `inputProps`: accepted prop contract.
- `compositionConstraints`: required/forbidden structures and nesting invariants.
- `outputSemantics`: expected delegate-compatible payload semantics.

### Validation rules
- Unsupported component aliases MUST fail with `unsupported_component` diagnostics.
- Component composition constraints MUST be enforced before output emission.
- Equivalent component input semantics MUST compile to deterministic canonical output.

### Relationships
- Consumes `JsxPrimitiveDefinition` instances.
- Produces canonical fragments merged into `ProjectJsonPayload`.

---

## 6) CompilationDiagnostic

Structured compiler/validation diagnostic used for invalid JSX or invalid mixed-format project inputs.

### Core fields
- `code`: stable machine-readable diagnostic code.
- `message`: human-readable failure reason.
- `path`: location context for failing construct.
- `hint` (optional): remediation hint.
- `details` (optional): supplemental metadata.

### Validation rules
- MUST avoid secret/internal stack disclosure.
- MUST identify failing construct and reason for invalid fixture classes.
- MUST map to sanitized external error envelopes at HTTP boundary.

### Relationships
- Emitted by validation/compile boundaries for `AuthoringProjectInput`.
- Prevents transition to delegate submission when blocking.

---

## 7) JsonDelegateResult

Delegate response surface from Rust JSON conversion.

### Core fields
- `status`: `Success | CapabilityDisabled | ValidationError | ConversionError`.
- `project_bundle`: converted internal bundle when status is success; absent otherwise.
- `errors`: delegate issues (`code`, `observability_code`, `message`, optional `path`, optional `hint`).
- `warnings`: optional non-fatal issue list.

### Validation rules
- Status-specific issue codes MUST match delegate behavior.
- Error messages and hints MUST avoid secret echoing.

### Relationships
- Generated from `JsonDelegateRequest` processing.
- Consumed by caller for sanitized error handling and observability.

---

## 8) CanonicalContractDefinition

Shared contract artifact used to keep producer outputs and consumer deserialization aligned.

### Core fields
- `schemaRevision`: active revision string (`chat_story_v1`).
- `requiredSections`: delegate-required payload sections and invariants.
- `delegateCompatibilityRules`: accepted alias/default behaviors and conversion constraints.
- `fixtureCorpus`: valid and invalid fixture classes used in conformance testing.

### Validation rules
- MUST be consumed by producer and consumer conformance tests.
- MUST be single source for in-scope workflows to prevent contract drift.

### Relationships
- Governs validation of `ProjectJsonPayload`.
- Referenced by TypeScript and Rust test suites.

---

## State transitions

1. `AuthoringProjectInput` received at producer boundary.
2. Producer-side validation/compilation runs.
3. If invalid -> emit `CompilationDiagnostic`, return sanitized failure, stop.
4. If valid -> serialize into `ProjectJsonPayload`.
5. Build `JsonDelegateRequest` with schema revision `chat_story_v1`.
6. Delegate validation runs (schema revision gate, JSON parse, conversion).
7. If success -> `JsonDelegateResult.status=Success` and `project_bundle` is returned.
8. If validation/conversion fails -> delegate issues are surfaced and submission is blocked.