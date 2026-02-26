# Expression + Animation Runtime Plan (Typed Dynamic Properties)

This document proposes a full refactor of Lumen's property animation/expression system around **typed dynamic fields** on node structs:

- `Literal(T)`
- `Expr(...)`
- `Animation(...)`

instead of centralized string-path track/expression tables.

The design keeps a compiled dependency graph/property-handle table at the composition level for:

- expression reference resolution
- cycle detection
- caching
- keyframe expression evaluation
- component prop plumbing

This is the long-term architecture plan for the idea:

```rust
pub enum Dynamic<T> {
    Literal(T),
    Expr(Arc<TypedExpression<T>>),
    Animation(Arc<KeyframeTrack<T>>),
}
```

## Why This Architecture

### Problems with the current/centralized approach

- Property targeting is stringly-typed (`"transform.translate_x"`).
- Validation and mutation logic is split across helper functions (`is_valid_property_path`, `apply_property`, `static_property_value`).
- Runtime evaluation patches cloned nodes by path instead of evaluating typed fields.
- Keyframe value coercion happens late and is easy to get wrong.
- Component-local props and external references require a lot of symbolic-path resolution machinery anyway.

### Benefits of typed dynamic fields

- Strong compile-time typing: `Dynamic<f32>` vs runtime coercion.
- Better locality: dynamic behavior lives where the property lives.
- Cleaner node code: `self.width.eval(...)`.
- Easier derive-based reflection and editor metadata generation.
- Better ergonomics for adding dynamic support to new node fields.

## Design Principles

1. **Typed node fields, compiled global dependency graph**
   - Node structs store `Dynamic<T>`.
   - Composition compiles dependencies and handles once.

2. **Expressions remain AST-first, not parser-generic**
   - Keep one expression parser + AST.
   - Add typed wrappers / typed compilation for target property types.

3. **Evaluation is handle-based**
   - Expressions should not recursively borrow node structs directly.
   - They resolve property references to stable `PropertyHandle`s.

4. **Components flatten first, expressions resolve second**
   - Components are lowered to a flat runtime graph.
   - Expression symbolic references are resolved after flattening, into handles.

5. **No string-path execution at runtime**
   - String paths are authoring/compile-time only.
   - Runtime uses typed handles and compiled closures/tables.

## Scope

### In scope

- Typed `Dynamic<T>` runtime field model
- Typed keyframe tracks and typed expression wrappers
- Composition-level property handle registry and dependency graph
- Component prop virtual handles
- Symbolic expression resolution (`component.pos_y`, `shape.position.y`, `hero_box.shape.position.y`)
- Derive macro for property reflection map generation
- Migration strategy from current centralized binding map

### Out of scope (initial migration)

- Full editor UI migration details (covered separately)
- Arbitrary object-valued expressions (e.g. evaluating a whole `position` object)
- Generic support for dynamic enum fields (unless explicitly required)
- Optimizing every eval path before correctness is locked

## Terminology

- **Dynamic field**: a node struct field represented as `Dynamic<T>`
- **PropertyHandle**: stable runtime identifier for a single leaf property
- **VirtualPropertyHandle**: non-node property handle used for component props
- **Symbolic path**: string/identifier expression reference before resolution (e.g. `hero_box.shape.position.y`)
- **Resolved dependency graph**: DAG of property-handle dependencies used at runtime

## Proposed Core Types

## 1) Dynamic Field Type

Introduce a new type in a dedicated module (e.g. `src/dynamic.rs`):

```rust
pub enum Dynamic<T> {
    Literal(T),
    Expr(Arc<TypedExpression<T>>),
    Animation(Arc<TypedKeyframeTrack<T>>),
}
```

### Why `Dynamic<T>` instead of `PropertyValue<T>`

Lumen already has runtime `PropertyValue` as a polymorphic enum. Reusing the name would cause confusion.

Use `Dynamic<T>` consistently for node fields.

## 2) Typed Expressions (wrapper, not generic parser)

Keep the parser output AST untyped:

- `Expression` (existing AST and metadata)

Add a typed wrapper:

```rust
pub struct TypedExpression<T> {
    pub expression: Expression,
    _marker: PhantomData<T>,
}
```

### Why this shape

- Avoids making the parser generic over `T`
- Preserves one parser and one AST
- Lets us add typed `eval_*` implementations per target type

## 3) Typed Keyframe Tracks

Refactor keyframes to be typed:

```rust
pub enum KeyValue<T> {
    Literal(T),
    Expr(Arc<TypedExpression<T>>),
}

pub struct TypedKeyframe<T> {
    pub frame: u32,
    pub value: KeyValue<T>,
    pub interpolation: InterpolationMode,
}

pub struct TypedKeyframeTrack<T> {
    pub keys: Vec<TypedKeyframe<T>>,
    pub before_extrapolation: Extrapolation,
    pub after_extrapolation: Extrapolation,
}
```

### Keyframe semantics (locked)

- `step`: evaluate selected key's value
- `linear`: evaluate left/right key values, then interpolate
- keyframe expressions evaluate at the **keyframe's own frame**
- `default_value` extrapolation uses property metadata default for the target

## 4) Property Handles and Registry

Introduce a runtime property registry in `Composition`.

```rust
pub struct PropertyHandle(u64);
pub struct VirtualPropertyHandle(u64);

pub enum ResolvedPropertyHandle {
    Node(PropertyHandle),
    Virtual(VirtualPropertyHandle),
}
```

Each registered property stores metadata:

```rust
pub struct PropertyMeta {
    pub debug_path: String,              // e.g. "hero_box.shape.position.y"
    pub value_type: AnimatableType,
    pub owner_node: Option<NodeId>,      // None for component virtual props
    pub supports_expression: bool,
    pub supports_animation: bool,
    pub aliases: &'static [&'static str], // optional canonicalization aliases
}
```

## 5) Compiled Property Slot Access

We need compiled get/set/eval access, not string matching.

Use function pointers or generated accessors:

```rust
pub struct PropertyAccessor {
    pub get_static: fn(&NodeKind) -> Option<PropertyValue>,
    pub apply_value: fn(&mut NodeKind, PropertyValue),
}
```

Long-term, we should prefer **typed accessors** to avoid boxing/coercion on hot paths:

```rust
pub struct TypedPropertyAccessor<T> {
    pub get_dynamic: fn(&NodeKind) -> Option<&Dynamic<T>>,
    pub get_dynamic_mut: fn(&mut NodeKind) -> Option<&mut Dynamic<T>>,
}
```

We can bridge typed accessors into a type-erased registry using generated adapters.

## Trait Model for `Dynamic<T>::eval`

`Dynamic<T>::eval(...)` is a good API, but it cannot be standalone. It needs composition services.

## 1) Eval Context

Introduce a dedicated property evaluation context:

```rust
pub struct PropertyEvalCtx<'a> {
    pub composition: &'a Composition,
    pub render_ctx: &'a RenderContext,
    pub frame: u32,
    pub frame_override: Option<u32>,
    pub active_stack: Vec<ResolvedPropertyHandle>, // or HashSet + stack
    pub cache: PropertyEvalCache,
}
```

### Responsibilities

- evaluate properties by handle
- prevent recursion/cycles
- memoize per-frame property results when safe
- support keyframe-expression frame override
- expose globals (`frame`, `time`, `fps`, `width`, `height`)

## 2) Type Constraints

Define traits for types that can live in `Dynamic<T>`:

```rust
pub trait DynamicValue:
    Clone + Send + Sync + 'static
{
    const VALUE_TYPE: AnimatableType;
    fn to_property_value(&self) -> PropertyValue;
    fn from_property_value(value: PropertyValue) -> Result<Self, LumenError>;
}

pub trait InterpolableDynamicValue: DynamicValue {
    fn interpolate(left: &Self, right: &Self, t: f64) -> Self;
}

pub trait ExpressionCoercible: DynamicValue {
    fn coerce_expression_value(value: &ExpressionValue) -> Result<Self, LumenError>;
}
```

### Examples

- `f32`, `f64`, `u32`, `i64`, `bool`, `String` -> `DynamicValue`
- `f32`, `u32`, `Color`, maybe `Vec2` -> `InterpolableDynamicValue`
- scalar/string types -> `ExpressionCoercible`

## 3) `Dynamic<T>::eval`

Proposed API:

```rust
impl<T> Dynamic<T>
where
    T: DynamicValue + ExpressionCoercible,
{
    pub fn eval(
        &self,
        target: ResolvedPropertyHandle,
        ctx: &mut PropertyEvalCtx<'_>,
    ) -> Result<T, LumenError>;
}
```

For animations with linear interpolation:

```rust
impl<T> Dynamic<T>
where
    T: DynamicValue + ExpressionCoercible + InterpolableDynamicValue,
{
    // same method, but linear interpolation is enabled
}
```

If a field type is not interpolable, validation rejects `linear` keys.

## Expression System Plan (Typed + Resolved)

## 1) Keep one parser / AST

Current parser and AST remain the canonical parsed form:

- parse string -> `Expression`
- expression may temporarily contain symbolic refs (`ExprNode::SymbolicPath`)

## 2) Add resolved references

Expression compilation step replaces symbolic refs with:

- `ExprNode::Property(PropertyHandle)`
- `ExprNode::VirtualProperty(VirtualPropertyHandle)`

The AST node variants can be:

```rust
ExprNode::Property(PropertyHandle)
ExprNode::VirtualProperty(VirtualPropertyHandle)
ExprNode::SymbolicPath(Vec<String>) // compile-time only; invalid after resolution
```

## 3) Typed expression wrappers compile against a target type

`TypedExpression<T>` wraps a resolved `Expression` and uses `T::coerce_expression_value(...)`.

This avoids generic ASTs and lets us compile once, evaluate many times.

## 4) Expression eval changes

Expression evaluation should read property values through `PropertyEvalCtx`:

- `ctx.eval_property(handle)` for node properties
- `ctx.eval_virtual_property(handle)` for component props

No direct string-path property lookups at runtime.

## Component Interaction Plan

## 1) Components still flatten at load time

Keep the current decision:

- nested components: allowed
- recursive components: rejected
- no runtime nested component node execution

## 2) Component props become virtual properties

When lowering a component instance:

- each prop allocates a `VirtualPropertyHandle`
- instance prop values are parsed as `Dynamic<T>`
- internal `component.<prop>` expression refs resolve to those virtual handles

## 3) External refs into component internals

Examples:

- `hero_box.shape.position.y`

Resolution process:

1. Parse symbolic path
2. Component lowering maps symbolic node paths to flattened node IDs
3. Property resolver maps `(flattened node + property path)` to `PropertyHandle`
4. Expression stores the handle

## Property Registry + Reflection (derive macro plan)

This is the key enabler for removing hand-written path logic.

## 1) Derive macro goal

Create a derive macro (likely in a new crate, e.g. `crates/lumen-macros`) that generates:

- canonical property list for a node struct
- path aliases
- property type metadata
- typed accessors
- optional editor metadata hooks

### Candidate derives

- `#[derive(LumenProperties)]` for node payload structs (`Transform`, `Resize`, etc.)
- optional field attributes:
  - `#[lumen(path = "translate_x")]`
  - `#[lumen(alias = "transform.translate_x")]`
  - `#[lumen(dynamic)]`
  - `#[lumen(no_expr)]`
  - `#[lumen(no_anim)]`

## 2) Generated output (conceptual)

For `Transform`:

- property descriptors for:
  - `scale_x`
  - `scale_y`
  - `translate_x`
  - `translate_y`
  - `rotate`
  - `pivot_x`
  - `pivot_y`
- aliases like `transform.translate_x`
- typed getters/setters for `Dynamic<f32>`

## 3) NodeKind integration

`NodeKind` gets a reflection entrypoint:

```rust
impl NodeKind {
    pub fn property_schema(&self) -> &'static [ErasedPropertyDescriptor];
    pub fn resolve_property(&self, path: &str) -> Option<ResolvedNodePropertyDescriptor>;
}
```

This replaces:

- `is_valid_property_path(...)`
- `expected_animatable_type(...)`
- `static_property_value(...)`
- `apply_property(...)`
- ad hoc path canonicalization

## Runtime Evaluation Flow (Target Architecture)

## 1) Composition build/load

1. Parse JSON (v2)
2. Lower components -> flat graph + symbolic node path map
3. Deserialize node structs with `Dynamic<T>` fields (typed)
4. Build property registry:
   - allocate `PropertyHandle`s for all dynamic-capable leaf properties
   - allocate `VirtualPropertyHandle`s for component props
5. Resolve expression symbolic refs -> property handles
6. Build dependency graph between property handles
7. Validate:
   - unresolved refs
   - type mismatches
   - invalid interpolation
   - dependency cycles
8. Freeze composition

## 2) Frame render

1. Build `PropertyEvalCtx` for frame
2. Evaluate nodes in graph order
3. Node uses typed dynamic fields directly:
   - `let width = self.width.eval(handle, &mut property_ctx)?;`
4. Property eval recursively resolves dependencies by handle
5. Property results may be cached per frame

## Caching Strategy

## 1) Property result cache

Introduce a per-frame cache in `PropertyEvalCtx`:

- key: `ResolvedPropertyHandle`
- value: typed-erased `PropertyValue` (or typed storage per type bucket)

Start with type-erased `PropertyValue` for simplicity, optimize later.

## 2) Frame dependence detection

Memo/memoization logic needs to know whether a property can vary by frame.

Compile and store per-property flags:

- depends on `frame`/`time`
- depends on another frame-varying property

This can be derived from the property dependency graph after expression resolution.

## 3) Interaction with `Memo` node

Memo eligibility should query compiled property metadata instead of scanning legacy track/expression tables.

## JSON / Serialization Plan

## 1) JSON remains inline-dynamic (v2)

Keep the v2 JSON shape:

- raw scalar literal
- `{ "expr": "..." }`
- `{ "anim": { ... } }`

This maps cleanly to `Dynamic<T>`.

## 2) Parsing strategy

For each dynamic-capable field:

- deserialize into intermediate `JsonDynamic<T>` OR parse from raw `serde_json::Value`
- compile into `Dynamic<T>`

Suggested helper:

```rust
enum JsonDynamic<T> {
    Literal(T),
    Expr(String),
    Animation(JsonTypedKeyframeTrack<T>),
}
```

but use targeted parsing if serde ergonomics get too expensive.

## 3) Keyframe values

For typed keyframe tracks:

- key value is `T` or `{ "expr": "..." }`
- compile to `KeyValue<T>`

## Migration Plan (Execution Order)

This is the recommended implementation order to keep the codebase buildable.

## Phase 0 - Preparation (no behavior change)

- Add `src/dynamic.rs` with `Dynamic<T>` and trait skeletons
- Add typed keyframe track types (parallel to legacy)
- Add typed expression wrapper types
- Add `PropertyHandle` / `VirtualPropertyHandle` types
- Add tests for coercion/interpolation helpers

**Goal:** introduce primitives without touching node runtime yet.

## Phase 1 - Property reflection infrastructure

- Create derive macro crate (`lumen-macros`) or internal proc-macro package
- Implement `#[derive(LumenProperties)]` for 1-2 node structs first:
  - `Transform`
  - `Shape`
- Add `NodeKind` property registry bridge
- Replace current hardcoded path validation/apply/default helpers for those nodes only

**Goal:** prove reflection approach works end-to-end.

## Phase 2 - Composition property registry + dependency graph

- Add composition-level property registry:
  - node property handles
  - virtual component prop handles
- Add expression symbolic resolution -> handles
- Add dependency graph build + cycle detection
- Add `PropertyEvalCtx`

**Goal:** compile all property references and validate dependencies once.

## Phase 3 - `Dynamic<T>::eval` integration for selected nodes

- Convert `Transform` fields to `Dynamic<f32>`
- Convert `Shape.position.{x,y}` and shape geometry dimensions where supported
- Update render paths to call `eval(...)` directly
- Keep fallback bridge for legacy centralized bindings (temporary)

**Goal:** render works through typed dynamic fields for the most used animated nodes.

## Phase 4 - Migrate JSON converter to typed dynamic fields

- JSON v2 parser compiles directly into `Dynamic<T>` on node structs
- Component props compile into virtual handles + `Dynamic<T>`
- Remove dependence on centralized `dynamic_bindings` for migrated nodes

**Goal:** new JSON path populates typed dynamic fields directly.

## Phase 5 - Full node migration

Convert remaining dynamic-capable fields across node types:

- `Resize.width`, `Resize.height`
- `Crop` fields (if dynamic support desired)
- `Merge.opacity`
- `SolidColor.width/height`
- text/vector text numeric fields as applicable
- shadow/blur/raster merge parameters as applicable

**Goal:** no runtime string-path evaluation for supported properties.

## Phase 6 - Remove legacy path-based runtime

Delete:

- centralized track/expression tables for runtime execution
- string-path apply helpers
- old `sample_property_without_expressions` execution path (retain debug/introspection wrapper if useful)

Keep only:

- property registry + handles
- typed dynamic fields
- compiled expression refs

## Compatibility / Bridging Strategy

Because this is a deep refactor, keep a transition layer during migration:

- `Composition.dynamic_bindings` (legacy/intermediate)
- `Composition.tracks` / `Composition.expressions` (legacy)
- typed `Dynamic<T>` fields on migrated nodes

Bridge options:

1. **Temporary adapter-in**
   - Populate legacy dynamic bindings from typed fields
   - Uses old runtime path for non-migrated nodes

2. **Temporary adapter-out**
   - Populate typed dynamic fields from legacy parsed bindings for migrated nodes

Prefer **adapter-out** once Phase 3 starts, because it exercises the new runtime path sooner.

## Validation Plan

Validation should happen after component lowering + expression resolution and before rendering.

## Required checks

- unresolved symbolic refs (`ExprNode::SymbolicPath` remaining after compile)
- invalid property path for target node
- target property type mismatch
- unsupported expression coercion target (`Color`, `Vector2`, etc. if disallowed)
- invalid interpolation mode for target type
- duplicate keyframe frames
- unsorted keys
- component recursion cycles
- dynamic dependency cycles (including virtual props)

## Testing Plan

## Unit tests

### Dynamic / typed keyframes

- `Dynamic<f32>::eval` literal/expr/anim
- keyframe expr evaluated at keyframe frame
- linear interpolation for numeric/color types
- invalid linear interpolation rejected for boolean/string

### Expression resolution

- `component.pos_y`
- `shape.position.y` (local)
- `hero_box.shape.position.y` (external)
- unresolved symbolic ref rejection

### Dependency graph

- direct cycle (`a -> b -> a`)
- transitive cycle (`a -> b -> c -> a`)
- self-cycle

## Integration tests (Rust)

- Component with animated prop driving internal shape position
- External transform expression reading internal component property
- Nested components (2+ levels)
- Recursive component ref rejected
- Memo eligibility respects compiled property frame-dependence

## Performance checks

- Compare frame render time before/after for common fixtures
- Measure property eval cache hit rate on expression-heavy scenes
- Ensure no major regressions in non-animated graphs

## Files / Modules to Introduce or Refactor

## New modules (proposed)

- `crates/lumen/src/dynamic.rs`
- `crates/lumen/src/property_registry.rs`
- `crates/lumen/src/property_eval.rs`
- `crates/lumen/src/property_handles.rs`
- `crates/lumen/src/json/dynamic_parse.rs` (optional helper split)
- `crates/lumen-macros/` (new proc-macro crate)

## Existing modules to refactor

- `crates/lumen/src/animation.rs` (typed keyframe tracks)
- `crates/lumen/src/expr/ast.rs` (resolved handle refs)
- `crates/lumen/src/expr/eval.rs` (handle-based property reads)
- `crates/lumen/src/composition.rs` (property registry + dependency graph + validation)
- `crates/lumen/src/json/convert.rs` (compile `Dynamic<T>` + resolve refs)
- `crates/lumen/src/node/*` (dynamic field conversion per node)
- `crates/lumen/src/render.rs` (`PropertyEvalCtx` integration)

## Risks and Mitigations

## Risk 1: Generic complexity explosion

**Mitigation**

- Keep expression AST untyped
- Use typed wrappers + trait-based coercion/interpolation
- Start with a small set of scalar types

## Risk 2: Borrow checker friction during recursive property evaluation

**Mitigation**

- Evaluate by `PropertyHandle` through `PropertyEvalCtx`
- Avoid direct recursive borrowing of node struct fields
- Store immutable compiled metadata separate from mutable render state

## Risk 3: Macro complexity / maintenance cost

**Mitigation**

- Start with a minimal derive macro generating static metadata + accessors
- Keep hand-written implementations as fallback for early rollout
- Add snapshot tests for generated property maps

## Risk 4: Partial migration confusion

**Mitigation**

- Explicitly mark nodes as "typed dynamic migrated" vs "legacy path"
- Keep tests for both paths during transition
- Remove legacy runtime only after parity is proven

## Success Criteria

We consider this refactor successful when:

- animated/expression fields are stored directly as `Dynamic<T>` in migrated nodes
- expressions resolve to property handles (no runtime symbolic/path lookup)
- component props and internal/external refs work through the same handle graph
- dynamic dependency cycles are rejected pre-render
- `Dynamic<T>::eval` is the primary property evaluation path in node code
- centralized string-path runtime patching is removed for migrated nodes (and ultimately all nodes)

## Recommended First Execution Slice

If starting implementation immediately, do this exact slice first:

1. Add `Dynamic<T>` + typed keyframe types + traits
2. Add `PropertyHandle` / `VirtualPropertyHandle`
3. Convert `Transform` fields to `Dynamic<f32>`
4. Implement `PropertyEvalCtx` + handle-based eval for `Transform`
5. Add minimal property registry (hand-written) for `Transform`
6. Resolve expressions to handles for `Transform` refs
7. Add tests for:
   - `transform.translate_x` literal/expr/anim
   - keyframe expr at keyframe frame
   - direct cycle detection

This gives us a clean, testable vertical slice before pulling in the derive macro and broad node migration.

