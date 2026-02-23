# Tasks: Lumen JSON Format Migration

**Input**: Design documents from `/specs/001-lumen-json-migration/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Include test tasks for every behavior-changing user story (US1, US2, US3).

**Organization**: Tasks are grouped by user story so each story can be implemented and validated independently.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on incomplete tasks)
- **[Story]**: User story label (`[US1]`, `[US2]`, `[US3]`)
- Every task includes an exact file path

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Initialize migration scaffolding and fixture inventory used by all stories.

- [X] T001 Add delegate-aligned canonical payload fixture baseline in packages/lumen/src/contracts/fixtures/renderer-contract.v1.json
- [X] T002 Define explicit JSX primitive/component inventory fixture set in packages/lumen-jsx/src/jsx.contract.test.ts
- [X] T003 [P] Add canonical delegate request envelope helper scaffolding in apps/editor/src/lib/lumen-client.ts
- [X] T004 [P] Add shared Rust-side delegate sample fixture coverage seed in crates/lumen/tests/fixtures/json_delegate/sample_project.json

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Build shared contract/validation foundations that block all user stories.

**⚠️ CRITICAL**: No user story implementation starts before this phase is complete.

- [X] T005 Define shared delegate request schema guard in packages/lumen/src/contracts/renderer-contract.ts
- [X] T006 Define shared payload conformance helper for producer workflows in packages/lumen/src/compile.ts
- [X] T007 [P] Add canonical fixture loader updates for delegate-aligned corpus in packages/lumen/src/contracts/renderer-contract.ts
- [X] T008 [P] Add baseline Rust JSON delegate fixture coverage in crates/lumen/src/json/tests.rs
- [X] T009 Implement sanitized delegate error mapping utility for producer-side failures in apps/editor/src/lib/lumen-client.ts
- [X] T010 Implement shared authorization guard for renderer-bound submission entrypoints in apps/editor/src/lib/lumen-client.ts

**Checkpoint**: Shared validation and delegate conformance foundation complete.

---

## Phase 3: User Story 1 - Publish projects in the new JSON contract (Priority: P1) 🎯 MVP

**Goal**: All four producer workflows emit payloads aligned with the delegate-compatible canonical JSON contract.

**Independent Test**: Generate payloads from editor, lumen, lumen-jsx, and templates and verify they conform to delegate-aligned canonical fixture expectations with no transformation layer.

### Tests for User Story 1

- [ ] T011 [P] [US1] Add producer contract parity test for delegate payload shape in packages/lumen/src/compile.parity.test.ts
- [ ] T012 [P] [US1] Add editor export contract test for canonical payload shape in apps/editor/src/preview/project-schema.ts
- [ ] T013 [P] [US1] Add templates preset output contract test for canonical payload shape in packages/templates/src/presets/chat-story-v1/chat-story.contract.test.ts
- [ ] T014 [P] [US1] Add Rust delegate conformance tests for valid canonical payload and schema revision mismatch in crates/lumen/src/json/tests.rs

### Implementation for User Story 1

- [ ] T015 [US1] Update editor payload serialization to emit delegate-aligned canonical fields in apps/editor/src/preview/project-schema.ts
- [ ] T016 [US1] Update lumen compile contract output to canonical delegate payload structure in packages/lumen/src/compile.ts
- [ ] T017 [US1] Update JSX serialization output mapping to delegate payload structure in packages/lumen-jsx/src/jsx.ts
- [ ] T018 [US1] Update templates preset builder output to delegate payload structure in packages/templates/src/presets/chat-story-v1/source-builder.ts
- [ ] T019 [US1] Wire canonical payload submission path through editor client request envelope in apps/editor/src/lib/lumen-client.ts

**Checkpoint**: US1 independently delivers canonical payload publishing across all workflows.

---

## Phase 4: User Story 2 - Remove dependency on legacy versioned project format (Priority: P2)

**Goal**: Remove legacy versioned branching in touched flows and fail closed for legacy/mixed payloads.

**Independent Test**: Verify no touched producer path emits or branches on legacy version markers; legacy-only and mixed payloads are rejected with explicit invalid-format outcomes.

### Tests for User Story 2

- [ ] T020 [P] [US2] Add invalid legacy payload rejection test in packages/lumen/src/compile.parity.test.ts
- [ ] T021 [P] [US2] Add mixed payload rejection test in apps/editor/src/preview/project-schema.ts
- [ ] T022 [P] [US2] Add unauthorized renderer-submission rejection test in apps/editor/src/lib/lumen-client.test.ts

### Implementation for User Story 2

- [ ] T023 [US2] Remove legacy versioned compatibility branching in editor preview schema handling in apps/editor/src/preview/use-preview-helpers.ts
- [ ] T024 [US2] Remove legacy format handling branches in lumen compile pipeline in packages/lumen/src/pipeline.ts
- [ ] T025 [US2] Enforce strict mixed/legacy rejection in template validation path in packages/templates/src/presets/chat-story-v1/validation.ts
- [ ] T026 [US2] Enforce authorization checks and sanitized invalid payload failures on submission path in apps/editor/src/lib/lumen-client.ts

**Checkpoint**: US2 independently enforces full cutover away from legacy versioned format.

---

## Phase 5: User Story 3 - Complete JSX rewrite coverage for components and primitives (Priority: P3)

**Goal**: JSX rewrite fully covers required primitives/components with deterministic output and actionable diagnostics.

**Independent Test**: Compile representative JSX fixtures with required primitives/components and invalid cases; valid cases compile to canonical payloads, invalid cases return typed diagnostics.

### Tests for User Story 3

- [ ] T027 [P] [US3] Add supported primitive/component fixture coverage in packages/lumen-jsx/src/jsx.contract.test.ts
- [ ] T028 [US3] Add unsupported primitive/component diagnostic coverage in packages/lumen-jsx/src/jsx.contract.test.ts
- [ ] T029 [P] [US3] Add deterministic output parity test for equivalent JSX inputs in packages/lumen/src/compile.parity.test.ts

### Implementation for User Story 3

- [ ] T030 [US3] Implement required primitive mapping and composition constraint enforcement in packages/lumen-jsx/src/jsx.ts
- [ ] T031 [US3] Update exported JSX components/primitives contract surface in packages/lumen-jsx/src/components/index.ts
- [ ] T032 [US3] Implement typed compile diagnostic envelope and failure classes in packages/lumen-jsx/src/jsx.ts
- [ ] T033 [US3] Align JSX runtime emission with deterministic canonical output ordering in packages/lumen-jsx/src/jsx-runtime.ts
- [ ] T034 [US3] Update template JSX usage to conform to rewritten primitive/component contracts in packages/templates/src/presets/chat-story-v1/index.tsx

**Checkpoint**: US3 independently delivers JSX rewrite parity, diagnostics, and determinism.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Final cross-story hardening and validation.

- [ ] T035 [P] Add export latency regression assertion (p95 within 5% baseline) in packages/lumen/src/compile.parity.test.ts
- [ ] T036 Map SC-001..SC-006 success criteria to concrete test evidence in specs/001-lumen-json-migration/quickstart.md
- [ ] T037 Run targeted workspace test commands from quickstart in specs/001-lumen-json-migration/quickstart.md

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies.
- **Phase 2 (Foundational)**: Depends on Phase 1; blocks all user stories.
- **Phase 3 (US1)**: Depends on Phase 2; defines MVP.
- **Phase 4 (US2)**: Depends on Phase 2 and can run after US1 starts; may reuse US1 artifacts.
- **Phase 5 (US3)**: Depends on Phase 2 and can run in parallel with US2 after shared fixtures are stable.
- **Phase 6 (Polish)**: Depends on completion of selected user stories.

### User Story Dependency Graph

- **US1 (P1)**: First delivery slice (MVP), no dependency on US2/US3.
- **US2 (P2)**: Depends on foundational conformance helpers; independent of US3.
- **US3 (P3)**: Depends on foundational conformance helpers; independent of US2.

Graph: `Setup -> Foundational -> {US1, US2, US3} -> Polish`

### Within-Story Execution Order

- Write story tests first and confirm they fail.
- Implement serialization/validation/compiler changes.
- Re-run story tests and confirm pass.
- Validate independent test criteria before moving to next story.

## Parallel Execution Examples

### US1 Parallel Example

```bash
# Parallel test work
T011 packages/lumen/src/compile.parity.test.ts
T012 apps/editor/src/preview/project-schema.ts
T013 packages/templates/src/presets/chat-story-v1/chat-story.contract.test.ts
T014 crates/lumen/src/json/tests.rs
```

### US2 Parallel Example

```bash
# Parallel rejection-path tests and implementation prep
T020 packages/lumen/src/compile.parity.test.ts
T021 apps/editor/src/preview/project-schema.ts
T022 crates/lumen/src/json/tests.rs
```

### US3 Parallel Example

```bash
# Parallel JSX contract and determinism test work
T027 packages/lumen-jsx/src/jsx.contract.test.ts
T028 packages/lumen-jsx/src/jsx.contract.test.ts
T029 packages/lumen/src/compile.parity.test.ts
```

## Implementation Strategy

### MVP First (US1 Only)

1. Complete Phase 1 and Phase 2.
2. Complete US1 (Phase 3).
3. Validate US1 independent test criteria.
4. Stop for MVP review/demo.

### Incremental Delivery

1. Deliver US1 canonical publishing.
2. Deliver US2 legacy cutover enforcement.
3. Deliver US3 JSX rewrite parity and diagnostics.
4. Finish with Polish phase and full targeted verification.

### Team Parallelization

1. Team completes Setup + Foundational together.
2. After Foundational:
   - Engineer A: US1
   - Engineer B: US2
   - Engineer C: US3
3. Integrate and run Polish tasks.

## Notes

- [P] tasks operate on independent files or independent test targets.
- Every story phase is independently testable using its own criteria.
- Suggested MVP scope: **US1 only**.
- Prefer full cutover in touched paths; no dual-path legacy shims.