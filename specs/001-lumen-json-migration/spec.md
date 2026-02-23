# Feature Specification: Lumen JSON Format Migration

**Feature Branch**: `001-lumen-json-migration`  
**Created**: 2026-02-22  
**Status**: Draft  
**Input**: User description: "a migration for apps/editor packages/lumen packages/lumen-jsx and packages/templates to use the JSON format required by crates/lumen under the JSON feature flag. Ive noticed that Project was previously versioned. we dont need to worry about supporting that because this is our PR for the rewrite and that was a preview."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Publish projects in the new JSON contract (Priority: P1)

As a product team shipping projects from the editor and shared packages, I need every project payload to match the new JSON contract so rendering requests are accepted consistently.

**Why this priority**: If outgoing payloads do not match the new contract, the core rendering flow fails regardless of other improvements.

**Independent Test**: Generate project payloads from each in-scope package and validate they conform to the new contract without any transformation layer.

**Acceptance Scenarios**:

1. **Given** a project created in the editor, **When** it is exported for rendering, **Then** the payload matches the new JSON contract expected by the renderer.
2. **Given** a project produced by shared authoring packages, **When** it is serialized, **Then** it uses the same field names, nesting, and required values as the new contract.

---

### User Story 2 - Remove dependency on legacy versioned project format (Priority: P2)

As a maintainer, I need the rewrite to stop handling preview-era versioned project payloads so the codebase has one canonical project format.

**Why this priority**: Maintaining parallel legacy format logic increases complexity and creates ambiguous behavior during the rewrite.

**Independent Test**: Review all in-scope packages and confirm there is no remaining behavior that emits, expects, or branches on legacy project version markers.

**Acceptance Scenarios**:

1. **Given** project serialization in any in-scope package, **When** payloads are generated, **Then** no legacy version wrapper or version-gated shape is produced.
2. **Given** project validation paths in the in-scope packages, **When** a legacy versioned payload is provided, **Then** it is rejected with a clear invalid-format result.

---

### User Story 3 - Complete JSX rewrite coverage for components and primitives (Priority: P3)

As a JSX author, I need the rewritten JSX path to cover required components, primitives, and compiler behaviors so authored projects compile into valid JSON without feature loss.

**Why this priority**: The migration is incomplete if JSX authors cannot express the full supported design surface or cannot diagnose compilation failures clearly.

**Independent Test**: Compile a representative JSX fixture suite covering required primitives, composite components, nesting patterns, and invalid inputs; verify valid fixtures produce canonical JSON and invalid fixtures produce explicit diagnostics.

**Acceptance Scenarios**:

1. **Given** a JSX project using supported primitives and components, **When** compilation runs, **Then** it outputs canonical JSON with equivalent authored structure and timing semantics.
2. **Given** a JSX project using unsupported primitives or invalid composition patterns, **When** compilation runs, **Then** it fails with explicit diagnostics that identify the offending node and reason.
3. **Given** two equivalent JSX inputs compiled in separate runs, **When** outputs are compared, **Then** the canonical JSON is deterministic for fields where ordering and identity are defined.

### Edge Cases

- Malformed or partially populated project inputs MUST fail validation with explicit invalid-format feedback rather than partial serialization.
- Unknown or preview-only project version metadata MUST be ignored for output generation and MUST NOT alter payload structure.
- Mixed inputs that combine new-format fields with legacy version wrappers MUST be treated as invalid to prevent ambiguous interpretation.
- JSX trees with unsupported primitives, unknown component aliases, or invalid nesting MUST fail compilation with actionable diagnostics.
- Compiler output MUST remain deterministic for equivalent JSX input so repeated compilation does not introduce non-semantic payload churn.

## Assumptions

- This rewrite has no requirement to read or preserve preview-era versioned project payloads.
- The renderer-side JSON delegate contract under the feature flag is the single source of truth for project shape and request envelope expectations.
- All migration work is limited to the editor workflow and the three shared authoring workflows (core project library, JSX authoring, and templates).
- JSX rewrite scope includes primitive coverage, component composition rules, and compiler diagnostics sufficient for teams to migrate existing authored content.
- Required primitives and components are defined by a maintained JSX rewrite inventory fixture set that is versioned with this feature scope.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST produce a single canonical project JSON shape across all in-scope packages.
- **FR-002**: System MUST prove compatibility of exported project JSON with the renderer JSON delegate consumer using shared conformance fixtures and status expectations.
- **FR-003**: System MUST remove legacy versioned project branching from all in-scope serialization and validation flows.
- **FR-004**: System MUST reject legacy-only or mixed-format project payloads with explicit invalid-format outcomes.
- **FR-005**: System MUST preserve authored project semantics (scenes, timing, media references, composition intent) when represented in the new JSON shape.
- **FR-006**: Users MUST be able to generate renderer-ready project JSON from editor, template, and JSX authoring workflows without manual post-processing.
- **FR-007**: System MUST define one shared contract reference for project JSON used by all in-scope workflows to prevent format drift.
- **FR-008**: System MUST define and maintain an explicit JSX primitive and component inventory for rewrite scope completion, and ensure each listed primitive/component compiles into canonical JSON.
- **FR-009**: System MUST define component composition constraints (allowed nesting, required properties, and structural invariants) for JSX authoring inputs.
- **FR-010**: System MUST provide deterministic JSX compilation outcomes for equivalent input so canonical JSON is stable across repeated compilations.
- **FR-011**: System MUST provide explicit compile diagnostics for JSX errors, including offending source location/context and a human-readable failure reason.
- **FR-012**: System MUST block renderer submission for JSX projects that fail canonical contract validation after compilation.

### Security and Boundary Requirements *(mandatory)*

- **SR-001**: System MUST treat project payloads from editor input, template input, and JSX input as external trust boundaries requiring schema validation before serialization.
- **SR-002**: System MUST fail closed on invalid or ambiguous project payloads and MUST NOT emit partially valid JSON outputs.
- **SR-003**: System MUST enforce authorization checks on all renderer-bound project serialization and submission entrypoints (editor export invocation, template build invocation, and JSX compile-to-render invocation), rejecting unauthorized calls before serialization occurs.
- **SR-004**: System MUST prevent sensitive internal diagnostics from being exposed in invalid-format responses; only sanitized error detail may be returned.

### Key Entities *(include if feature involves data)*

- **Project JSON Payload**: Canonical renderer-ready representation of a project, including scenes, timeline/composition data, and asset references.
- **Authoring Project Input**: Source project data created from editor, JSX, or template workflows before canonical JSON serialization.
- **JSX Primitive Definition**: Atomic authoring building block in JSX that maps to a canonical JSON construct with defined required/optional properties.
- **JSX Component Contract**: Higher-level reusable JSX construct that composes primitives under defined structural constraints and compilation invariants.
- **Compilation Diagnostic**: Structured compile-time failure or warning describing invalid JSX input, source context, and remediation guidance.
- **Legacy Versioned Project**: Preview-era project representation that is now out of scope and must not be emitted or supported in rewrite flows.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of project payloads generated by the four in-scope workflows validate against the canonical JSON contract.
- **SC-002**: 0 production rewrite flows depend on legacy versioned project handling after migration completion.
- **SC-003**: 100% of required JSX primitives and components identified for rewrite scope compile successfully in acceptance fixtures.
- **SC-004**: At least 95% of representative existing template and JSX authoring fixtures produce equivalent render-intent outputs after migration.
- **SC-005**: 100% of invalid JSX fixture cases fail with diagnostics that identify both the failing construct and failure reason.
- **SC-006**: Teams can complete project export from each in-scope workflow without manual payload edits in 100% of acceptance test runs.