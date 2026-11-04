//! Custom WGSL compute shaders.
//!
//! The engine appends [`PRELUDE`] to the user's source. It declares, in bind group 0:
//!
//! | binding | name            | type                                       |
//! |---------|-----------------|--------------------------------------------|
//! | 0       | `input0`        | `texture_2d<f32>`                          |
//! | 1       | (user uniform)  | the shader's own `var<uniform>` struct     |
//! | 2       | `output`        | `texture_storage_2d<rgba8unorm, write>`    |
//! | 3       | `lumen`         | `LumenGlobals { resolution, time, frame }` |
//! | 4       | `input_sampler` | linear, clamp-to-edge `sampler`            |
//! | 4 + i   | `input{i}`      | `texture_2d<f32>`, for `i` in `1..N`       |
//!
//! A shader may declare bindings 0 and 2 itself under its own names, and the prelude then leaves
//! them out; bindings 3 and up belong to the prelude. The entry point is `cs_main` with
//! `@workgroup_size(8, 8, 1)`, dispatched over input0's size.
//!
//! Every input is bound as premultiplied alpha, and the output is treated as premultiplied alpha.
//!
//! Uniform fields are read from the struct behind the shader's single `var<uniform>`, laid out
//! with WGSL's uniform rules, and set by name through `uniforms`. Unset fields are zero.

use crate::{
    error::RenderError,
    expr::{ExpressionContext, ExpressionValue},
    node::{
        Deferred, DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, ShaderUniformValue,
        ShaderUniforms,
    },
};

use crate::gpu::params;

/// WebGPU's default `maxSampledTexturesPerShaderStage`, guaranteed on every adapter.
pub const MAX_INPUTS: usize = 16;

const OUTPUT_BINDING: u32 = 2;

pub const PRELUDE: &str = "
// wgsl_shader prelude
// Every input is premultiplied alpha, and `output` is read as premultiplied alpha.
struct LumenGlobals {
    // Output size in pixels.
    resolution: vec2<f32>,
    // Seconds since the start of the composition: frame / fps.
    time: f32,
    // Composition frame index, rounded down at fractional frame times.
    frame: u32,
}
@group(0) @binding(3) var<uniform> lumen: LumenGlobals;
@group(0) @binding(4) var input_sampler: sampler;
";

const DEFAULT_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("wgsl_shader.wgsl")
);

/// Runs a custom WGSL compute shader over one or more rasters.
#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct WgslShaderParams {
    /// WGSL compute shader; the prelude provides inputs, `input_sampler`, `output` and `lumen`.
    #[meta(
        name = "Shader source",
        format = "wgsl",
        multiline,
        recommended_rows = 10
    )]
    pub shader: String,
    /// Uniform struct field values by name; unset fields are zero.
    #[meta(name = "Shader uniforms")]
    pub uniforms: ShaderUniforms,
}

/// Runs a custom WGSL compute shader over one or more rasters.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "wgsl_shader", name = "WGSL Shader", category = "processing")]
pub struct WgslShader {
    pub id: NodeId,
    #[params]
    pub params: WgslShaderParamsDelegate,
    /// Bound as `input0..inputN` in connection order.
    #[input(name = "source", variadic)]
    pub sources: Vec<NodeId>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LumenGlobals {
    resolution: [f32; 2],
    time: f32,
    frame: u32,
}

/// `input0` sits at binding 0; later inputs follow the prelude's sampler.
fn input_binding(index: usize) -> u32 {
    if index == 0 { 0 } else { 4 + index as u32 }
}

fn shader_error(node_id: NodeId, details: impl Into<String>) -> crate::error::LumenError {
    RenderError::NodeEvaluation {
        node_id,
        node_kind: "WgslShader",
        details: details.into(),
    }
    .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    F32,
    I32,
    U32,
}

#[derive(Debug, Clone)]
struct UniformField {
    name: String,
    scalar: Scalar,
    lanes: usize,
    offset: usize,
}

impl UniformField {
    fn wgsl_type(&self) -> String {
        let scalar = match self.scalar {
            Scalar::F32 => "f32",
            Scalar::I32 => "i32",
            Scalar::U32 => "u32",
        };
        match self.lanes {
            1 => scalar.to_string(),
            lanes => format!("vec{lanes}<{scalar}>"),
        }
    }
}

/// What the user's shader declares: the bindings it claims and its uniform struct layout.
#[derive(Debug, Clone)]
struct ShaderInterface {
    declares_input0: bool,
    declares_output: bool,
    struct_name: Option<String>,
    fields: Vec<UniformField>,
    uniform_size: usize,
}

impl ShaderInterface {
    fn parse(source: &str) -> Result<Self, String> {
        let source = strip_comments(source);
        let tokens = tokenize(&source);
        check_workgroup_sizes(&tokens)?;
        for group in attribute_values(&tokens, "group") {
            if group? != 0 {
                return Err("only `@group(0)` is available".to_string());
            }
        }
        let mut bindings = Vec::new();
        for binding in attribute_values(&tokens, "binding") {
            let binding = binding?;
            if binding > OUTPUT_BINDING {
                return Err(format!(
                    "`@binding({binding})` is reserved for the prelude; declare only bindings 0-2"
                ));
            }
            bindings.push(binding);
        }

        let mut uniforms = (0..tokens.len())
            .filter(|&index| tokens[index..].starts_with(&["var", "<", "uniform", ">"]))
            .map(|index| &tokens[index + 4..]);
        let mut interface = Self {
            declares_input0: bindings.contains(&0),
            declares_output: bindings.contains(&OUTPUT_BINDING),
            struct_name: None,
            fields: Vec::new(),
            uniform_size: 16,
        };
        let Some(declaration) = uniforms.next() else {
            return Ok(interface);
        };
        if uniforms.next().is_some() {
            return Err("declare at most one `var<uniform>`".to_string());
        }
        let struct_name = match declaration {
            [_, ":", rest @ ..] => rest
                .iter()
                .take_while(|&&token| token != ";")
                .copied()
                .collect::<String>(),
            _ => String::new(),
        };
        if struct_name.is_empty() {
            return Err("malformed `var<uniform>` declaration".to_string());
        }
        let body = struct_body(&tokens, &struct_name).ok_or_else(|| {
            format!(
                "the `var<uniform>` type `{struct_name}` must be a struct declared in the shader"
            )
        })?;

        let mut end = 0;
        for member in split_members(body) {
            let (name, ty) = match member {
                ["@", attribute, ..] => {
                    return Err(format!(
                        "uniform field attributes are not supported (`@{attribute}`)"
                    ));
                }
                [name, ":", ty @ ..] if !ty.is_empty() => (*name, ty.concat()),
                _ => return Err(format!("malformed uniform field `{}`", member.join(" "))),
            };
            let (scalar, lanes) = parse_type(&ty).ok_or_else(|| {
                format!(
                    "uniform field `{name}` has unsupported type `{ty}`; use f32, i32, u32 or \
                     vec2/vec3/vec4 of them"
                )
            })?;
            let align = match lanes {
                1 => 4,
                2 => 8,
                _ => 16,
            };
            let offset = usize::next_multiple_of(end, align);
            end = offset + 4 * lanes;
            interface.fields.push(UniformField {
                name: name.to_string(),
                scalar,
                lanes,
                offset,
            });
        }
        interface.uniform_size = end.next_multiple_of(16).max(16);
        let max_size = lumen_gpu::wgpu::Limits::default().max_uniform_buffer_binding_size;
        if interface.uniform_size as u64 > max_size {
            return Err(format!(
                "`{struct_name}` is {} bytes; uniform buffers are limited to {max_size} bytes",
                interface.uniform_size
            ));
        }
        interface.struct_name = Some(struct_name);
        Ok(interface)
    }

    /// The user's source followed by the prelude, which declares every binding the shader left out.
    fn assemble(&self, source: &str, inputs: usize) -> String {
        let mut shader = format!("{source}\n{PRELUDE}");
        if !self.declares_input0 {
            shader.push_str("@group(0) @binding(0) var input0: texture_2d<f32>;\n");
        }
        if !self.declares_output {
            shader.push_str(
                "@group(0) @binding(2) var output: texture_storage_2d<rgba8unorm, write>;\n",
            );
        }
        for index in 1..inputs {
            shader.push_str(&format!(
                "@group(0) @binding({}) var input{index}: texture_2d<f32>;\n",
                input_binding(index)
            ));
        }
        shader
    }

    fn uniform_bytes(
        &self,
        node_id: NodeId,
        uniforms: &ShaderUniforms,
        ctx: &ExpressionContext<'_>,
    ) -> crate::Result<Vec<u8>> {
        let mut bytes = vec![0; self.uniform_size];
        for (name, value) in &uniforms.0 {
            let Some(field) = self.fields.iter().find(|field| &field.name == name) else {
                let details = match &self.struct_name {
                    Some(struct_name) => format!(
                        "`{struct_name}` has no such field; it declares: {}",
                        self.fields
                            .iter()
                            .map(|field| field.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    None => "the shader declares no `var<uniform>`".to_string(),
                };
                return Err(shader_error(node_id, format!("uniforms.{name}: {details}")));
            };
            for (lane, value) in uniform_components(node_id, field, value, ctx)?
                .into_iter()
                .enumerate()
            {
                let raw = match field.scalar {
                    Scalar::F32 => (value as f32).to_le_bytes(),
                    Scalar::I32 => (value as i32).to_le_bytes(),
                    Scalar::U32 => (value as u32).to_le_bytes(),
                };
                let at = field.offset + lane * 4;
                bytes[at..at + 4].copy_from_slice(&raw);
            }
        }
        Ok(bytes)
    }
}

/// Resolves one uniform value into exactly `field.lanes` components that fit `field.scalar`.
fn uniform_components(
    node_id: NodeId,
    field: &UniformField,
    value: &ShaderUniformValue,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<Vec<f64>> {
    let invalid =
        |details: String| shader_error(node_id, format!("uniforms.{}: {details}", field.name));
    let located = |error: crate::error::LumenError| {
        error.located(node_id, &format!("uniforms.{}", field.name))
    };
    let number = |value: ExpressionValue| {
        value.as_f64().ok_or_else(|| {
            invalid(format!(
                "expression returned a {}, expected a number",
                value.type_name()
            ))
        })
    };
    let color_field = || {
        if field.scalar != Scalar::F32 || field.lanes < 3 {
            return Err(invalid(format!(
                "colours need a vec3<f32> or vec4<f32> field, not {}",
                field.wgsl_type()
            )));
        }
        Ok(())
    };

    let values = match value {
        ShaderUniformValue::Number(value) => vec![*value],
        ShaderUniformValue::Expr(expr) => match expr.evaluate(ctx).map_err(located)? {
            ExpressionValue::Vec2((x, y)) => vec![x, y],
            ExpressionValue::Color(rgba) => {
                color_field()?;
                if field.lanes == 3 && rgba[3] != 1.0 {
                    return Err(invalid("vec3<f32> colours must be opaque".to_string()));
                }
                rgba[..field.lanes].to_vec()
            }
            value => vec![number(value)?],
        },
        ShaderUniformValue::Vector(components) => components
            .iter()
            .map(|component| match component {
                Deferred::Value(value) => Ok(*value),
                Deferred::Expr(expr) => number(expr.evaluate(ctx).map_err(located)?),
            })
            .collect::<crate::Result<_>>()?,
        ShaderUniformValue::Color { rgb, alpha } => {
            color_field()?;
            if alpha.is_some() && field.lanes == 3 {
                return Err(invalid(
                    "vec3<f32> colours take `#rrggbb`, without alpha".to_string(),
                ));
            }
            let mut values: Vec<f64> = rgb
                .iter()
                .chain(alpha)
                .map(|&channel| f64::from(channel) / 255.0)
                .collect();
            if values.len() < field.lanes {
                values.push(1.0);
            }
            values
        }
    };

    if values.len() != field.lanes {
        return Err(invalid(format!(
            "{} needs {} components, got {}",
            field.wgsl_type(),
            field.lanes,
            values.len()
        )));
    }
    for &value in &values {
        let fits = match field.scalar {
            Scalar::F32 => (value as f32).is_finite(),
            Scalar::I32 => {
                value.fract() == 0.0 && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&value)
            }
            Scalar::U32 => value.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&value),
        };
        if !fits {
            return Err(invalid(format!(
                "{value} is not a valid {} component",
                field.wgsl_type()
            )));
        }
    }
    Ok(values)
}

fn parse_type(ty: &str) -> Option<(Scalar, usize)> {
    let scalar = |name: &str| match name {
        "f32" => Some(Scalar::F32),
        "i32" => Some(Scalar::I32),
        "u32" => Some(Scalar::U32),
        _ => None,
    };
    if let Some(scalar) = scalar(ty) {
        return Some((scalar, 1));
    }
    let rest = ty.strip_prefix("vec")?;
    let lanes = match rest.get(..1)? {
        "2" => 2,
        "3" => 3,
        "4" => 4,
        _ => return None,
    };
    let scalar = match &rest[1..] {
        "f" => Scalar::F32,
        "i" => Scalar::I32,
        "u" => Scalar::U32,
        rest => scalar(rest.strip_prefix('<')?.strip_suffix('>')?)?,
    };
    Some((scalar, lanes))
}

/// Splits comment-free WGSL into words (identifiers and literals) and single punctuation
/// characters, so declarations match regardless of whitespace.
fn tokenize(source: &str) -> Vec<&str> {
    let is_word = |char: char| char.is_alphanumeric() || char == '_';
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((start, char)) = chars.next() {
        if char.is_whitespace() {
            continue;
        }
        let mut end = start + char.len_utf8();
        if is_word(char) {
            while let Some((index, next)) = chars.next_if(|&(_, next)| is_word(next)) {
                end = index + next.len_utf8();
            }
        }
        tokens.push(&source[start..end]);
    }
    tokens
}

/// The tokens between the parentheses of every `@name(...)` attribute.
fn attribute_args<'a>(
    tokens: &'a [&'a str],
    name: &'a str,
) -> impl Iterator<Item = &'a [&'a str]> + 'a {
    tokens
        .windows(3)
        .enumerate()
        .filter_map(move |(index, window)| {
            if window != ["@", name, "("] {
                return None;
            }
            let args = &tokens[index + 3..];
            Some(&args[..args.iter().position(|&token| token == ")")?])
        })
}

fn integer_literal(token: &str) -> Option<u32> {
    token.trim_end_matches(['u', 'i']).parse().ok()
}

/// Integer literals of every `@name(...)` attribute; non-literal arguments are errors.
fn attribute_values<'a>(
    tokens: &'a [&'a str],
    name: &'a str,
) -> impl Iterator<Item = Result<u32, String>> + 'a {
    attribute_args(tokens, name).map(move |args| {
        match args {
            [literal] => integer_literal(literal),
            _ => None,
        }
        .ok_or_else(|| format!("`@{name}({})` must use an integer literal", args.concat()))
    })
}

/// Errors unless every `@workgroup_size(...)` is 8, 8, 1, the size the engine dispatches for.
fn check_workgroup_sizes(tokens: &[&str]) -> Result<(), String> {
    for args in attribute_args(tokens, "workgroup_size") {
        let sizes = args
            .split(|&token| token == ",")
            .filter(|size| !size.is_empty())
            .map(|size| match size {
                [literal] => integer_literal(literal),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .filter(|sizes| (1..=3).contains(&sizes.len()))
            .ok_or_else(|| {
                format!(
                    "`@workgroup_size({})` must use integer literals",
                    args.concat()
                )
            })?;
        let size = |axis: usize| sizes.get(axis).copied().unwrap_or(1);
        if [size(0), size(1), size(2)] != [8, 8, 1] {
            return Err(format!(
                "`@workgroup_size({})` must be `@workgroup_size(8, 8, 1)`; the engine dispatches \
                 one thread per pixel in 8x8 groups",
                args.concat()
            ));
        }
    }
    Ok(())
}

fn struct_body<'a>(tokens: &'a [&'a str], name: &str) -> Option<&'a [&'a str]> {
    let start =
        (0..tokens.len()).find(|&index| tokens[index..].starts_with(&["struct", name, "{"]))? + 3;
    let len = tokens[start..].iter().position(|&token| token == "}")?;
    Some(&tokens[start..start + len])
}

/// Struct members, split on commas outside `<...>` template arguments.
fn split_members<'a>(body: &'a [&'a str]) -> impl Iterator<Item = &'a [&'a str]> {
    let mut depth = 0i32;
    body.split(move |&token| {
        match token {
            "<" => depth += 1,
            ">" => depth -= 1,
            _ => {}
        }
        token == "," && depth == 0
    })
    .filter(|member| !member.is_empty())
}

/// Replaces `//` and (nested) `/* */` comments with whitespace.
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut depth = 0;
    while let Some(char) = chars.next() {
        let next = chars.peek().copied();
        match (char, next) {
            ('/', Some('*')) => {
                chars.next();
                depth += 1;
            }
            ('*', Some('/')) if depth > 0 => {
                chars.next();
                depth -= 1;
                out.push(' ');
            }
            ('/', Some('/')) if depth == 0 => {
                while chars.next_if(|&char| char != '\n').is_some() {}
            }
            _ if depth > 0 => {}
            _ => out.push(char),
        }
    }
    out
}

/// The parts a recorded frame caches a shader from `source` as, with `inputs` inputs.
fn record_parts(
    node_id: NodeId,
    source: &str,
    inputs: usize,
) -> crate::Result<crate::gpu::frame_graph::ShaderParts> {
    use lumen_gpu::{LayoutEntry, LayoutKind};

    let interface = ShaderInterface::parse(source)
        .map_err(|details| shader_error(node_id, format!("shader: {details}")))?;
    let assembled = interface.assemble(source, inputs);
    let mut layout = vec![
        LayoutEntry::compute(1, LayoutKind::Uniform),
        LayoutEntry::compute(
            OUTPUT_BINDING,
            LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        ),
        LayoutEntry::compute(3, LayoutKind::Uniform),
        LayoutEntry::compute(4, LayoutKind::Sampler),
    ];
    layout.extend(
        (0..inputs).map(|index| LayoutEntry::compute(input_binding(index), LayoutKind::Texture)),
    );
    Ok((std::sync::Arc::new(interface) as _, assembled, layout))
}

impl crate::gpu::frame_graph::RecordNode for WgslShader {
    /// A source that an expression picks builds when a frame first picks it.
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        let inputs = self.sources.len();
        let Deferred::Value(source) = &self.params.shader else {
            return Ok(());
        };
        if inputs == 0 || inputs > MAX_INPUTS {
            return Ok(());
        }
        let source = if source.trim().is_empty() {
            DEFAULT_SHADER
        } else {
            source.as_str()
        };
        programs.dynamic("wgsl-shader", source, inputs, || {
            record_parts(self.id, source, inputs)
        })
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        use lumen_gpu::Res;

        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let inputs = self
            .sources
            .iter()
            .map(|&source| cx.input_full(source))
            .collect::<crate::Result<Vec<_>>>()?;
        let Some(first) = inputs.first().copied() else {
            return Err(shader_error(self.id, "needs at least one input"));
        };
        if inputs.len() > MAX_INPUTS {
            return Err(shader_error(
                self.id,
                format!(
                    "has {} inputs; WebGPU allows at most {MAX_INPUTS}",
                    inputs.len()
                ),
            ));
        }
        let user_source = if params.shader.trim().is_empty() {
            DEFAULT_SHADER
        } else {
            params.shader.as_str()
        };
        let input_count = inputs.len();
        let shader = cx.cached_shader(user_source, input_count, || {
            record_parts(self.id, user_source, input_count)
        })?;
        let interface = shader
            .interface
            .downcast_ref::<ShaderInterface>()
            .expect("shader interface");
        let size = first.domain.storage_size;
        let (uniforms, globals) = {
            let expr = cx.expr();
            (
                interface
                    .uniform_bytes(self.id, &params.uniforms, &expr)?
                    .to_vec(),
                LumenGlobals {
                    resolution: [size.width as f32, size.height as f32],
                    time: expr.time_seconds() as f32,
                    frame: expr.frame as u32,
                },
            )
        };
        // A shader need not write every pixel, and what it leaves must not be another node's.
        let tex = cx.cleared_storage_target(size);
        let program = cx.dynamic_program("wgsl-shader", &shader)?;
        let uniform = cx.rec.uniform_bytes(&uniforms);
        let globals = cx.rec.uniform(&globals);
        let sampler = cx.sampler(4, || lumen_gpu::wgpu::SamplerDescriptor {
            address_mode_u: lumen_gpu::wgpu::AddressMode::ClampToEdge,
            address_mode_v: lumen_gpu::wgpu::AddressMode::ClampToEdge,
            address_mode_w: lumen_gpu::wgpu::AddressMode::ClampToEdge,
            mag_filter: lumen_gpu::wgpu::FilterMode::Linear,
            min_filter: lumen_gpu::wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut bindings = vec![
            (1, uniform),
            (OUTPUT_BINDING, Res::Tex(tex)),
            (3, globals),
            (4, Res::Sampler(sampler)),
        ];
        for (index, input) in inputs.iter().enumerate() {
            bindings.push((input_binding(index), Res::Tex(input.tex)));
        }
        cx.dispatch(program, &bindings, params::dispatch_for(size));
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain: first.domain,
            bounds: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context() -> ExpressionContext<'static> {
        ExpressionContext {
            frame: 0.0,
            fps: 30.0,
            width: 1,
            height: 1,
            graph: None,
            media: None,
            glyph: None,
            props: None,
        }
    }

    const SHADER: &str = "
        struct Params {
            amount: f32,   // offset 0
            tint: vec3<f32>, /* offset 16 */
            count: u32,    // offset 28
            offset: vec2f, // offset 32
        }
        @group(0) @binding(1) var<uniform> params: Params;
    ";

    #[test]
    fn uniform_struct_follows_wgsl_layout() {
        let interface = ShaderInterface::parse(SHADER).unwrap();
        let offsets: Vec<_> = interface
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.offset))
            .collect();
        assert_eq!(
            offsets,
            [("amount", 0), ("tint", 16), ("count", 28), ("offset", 32)]
        );
        assert_eq!(interface.uniform_size, 48);
        assert!(!interface.declares_input0 && !interface.declares_output);
    }

    #[test]
    fn declarations_parse_with_any_whitespace() {
        let interface = ShaderInterface::parse(
            "struct  Params{ tint :vec3 < f32 > ,alpha : f32 }\n\
             @ group( 0 ) @binding( 1u ) var< uniform > params : Params ;",
        )
        .unwrap();
        let offsets: Vec<_> = interface
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.offset))
            .collect();
        assert_eq!(offsets, [("tint", 0), ("alpha", 12)]);
        assert_eq!(interface.uniform_size, 16);
        let bytes = interface
            .uniform_bytes(
                NodeId::new(1),
                &uniforms(json!({ "tint": [0.5, 0.25, 1], "alpha": 1 })),
                &context(),
            )
            .unwrap();
        assert_eq!(
            bytemuck::cast_slice::<u8, f32>(&bytes),
            [0.5, 0.25, 1.0, 1.0]
        );
    }

    fn uniforms(value: serde_json::Value) -> ShaderUniforms {
        ShaderUniforms::from_json_value(&value, "uniforms").unwrap()
    }

    #[test]
    fn uniform_values_are_strict() {
        let node_id = NodeId::new(7);
        let ctx = context();
        let interface = ShaderInterface::parse(SHADER).unwrap();
        let bytes = interface
            .uniform_bytes(
                node_id,
                &uniforms(json!({ "tint": "#ff0033", "count": 3, "offset": ["=1 + 1", 0.5] })),
                &ctx,
            )
            .unwrap();
        let floats: &[f32] = bytemuck::cast_slice(&bytes);
        assert_eq!(floats[0], 0.0, "unset fields stay zero");
        assert_eq!(&floats[4..7], &[1.0, 0.0, 0.2]);
        assert_eq!(floats[8..10], [2.0, 0.5]);
        assert_eq!(bytemuck::cast_slice::<u8, u32>(&bytes)[7], 3);
        let bytes = interface
            .uniform_bytes(
                node_id,
                &uniforms(json!({ "amount": "=frame + 0.5", "offset": "=vec2(3, 4)" })),
                &ctx,
            )
            .unwrap();
        let floats: &[f32] = bytemuck::cast_slice(&bytes);
        assert_eq!((floats[0], &floats[8..10]), (0.5, &[3.0, 4.0][..]));
        let bytes = interface
            .uniform_bytes(
                node_id,
                &uniforms(json!({ "tint": "=rgb(255, 0, 51)" })),
                &ctx,
            )
            .unwrap();
        let floats: &[f32] = bytemuck::cast_slice(&bytes);
        assert_eq!(&floats[4..7], &[1.0, 0.0, 0.2]);

        for (value, expected) in [
            (
                json!({ "tint": [1, 0] }),
                "uniforms.tint: vec3<f32> needs 3 components, got 2",
            ),
            (json!({ "tint": "=frame" }), "needs 3 components, got 1"),
            (
                json!({ "offset": 1 }),
                "vec2<f32> needs 2 components, got 1",
            ),
            (json!({ "amount": 1e39 }), "uniforms.amount: 1"),
            (json!({ "amount": "=pow(10, 40)" }), "not a valid f32"),
            (json!({ "amount": "=pow(-1, 0.5)" }), "uniforms.amount"),
            (
                json!({ "amount": "='abc'" }),
                "uniforms.amount: expression returned a string, expected a number",
            ),
            (
                json!({ "offset": ["=vec2(1, 2)", 0] }),
                "uniforms.offset: expression returned a vec2",
            ),
            (
                json!({ "count": 1.5 }),
                "uniforms.count: 1.5 is not a valid u32 component",
            ),
            (
                json!({ "count": -1 }),
                "uniforms.count: -1 is not a valid u32",
            ),
            (
                json!({ "amount": "#ff0000" }),
                "uniforms.amount: colours need a vec3<f32> or vec4<f32> field, not f32",
            ),
            (
                json!({ "tint": "#ff000080" }),
                "uniforms.tint: vec3<f32> colours take `#rrggbb`, without alpha",
            ),
            (
                json!({ "tint": "=rgba(255, 0, 0, 128)" }),
                "uniforms.tint: vec3<f32> colours must be opaque",
            ),
            (
                json!({ "amount": "=rgb(255, 0, 0)" }),
                "uniforms.amount: colours need a vec3<f32> or vec4<f32> field, not f32",
            ),
            (
                json!({ "missing": 1 }),
                "uniforms.missing: `Params` has no such field; it declares: amount, tint, count, \
                 offset",
            ),
        ] {
            let error = interface
                .uniform_bytes(node_id, &uniforms(value.clone()), &ctx)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{value}: {error}");
        }

        let error = ShaderInterface::parse("")
            .unwrap()
            .uniform_bytes(node_id, &uniforms(json!({ "amount": 1 })), &ctx)
            .unwrap_err()
            .to_string();
        assert!(error.contains("declares no `var<uniform>`"), "{error}");
    }

    #[test]
    fn shader_interface_rejects_unsupported_declarations() {
        for (shader, expected) in [
            (
                "struct P { m: mat4x4<f32> } @group(0) @binding(1) var<uniform> p: P;",
                "unsupported type `mat4x4<f32>`",
            ),
            (
                "struct P { @align(16) a: f32 } @group(0) @binding(1) var<uniform> p: P;",
                "attributes are not supported",
            ),
            ("@group(0) @binding(5) var t: texture_2d<f32>;", "reserved"),
            ("@group(1) @binding(0) var t: texture_2d<f32>;", "@group(0)"),
            (
                "@group(0) @binding(1) var<uniform> p: Missing;",
                "`Missing`",
            ),
            (
                "@compute @workgroup_size(16) fn cs_main() {}",
                "must be `@workgroup_size(8, 8, 1)`",
            ),
            (
                "@compute @workgroup_size(8, 8) @workgroup_size(8, 4, 2) fn cs_main() {}",
                "@workgroup_size(8,4,2)",
            ),
        ] {
            let error = ShaderInterface::parse(shader).unwrap_err();
            assert!(error.contains(expected), "{shader}: {error}");
        }
        // Shaders may declare their own input and output; commented-out bindings are ignored.
        let interface = ShaderInterface::parse(
            "@group(0) @binding(0) var input_tex: texture_2d<f32>;
             // @group(0) @binding(9) var nope: sampler;
             @group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;",
        )
        .unwrap();
        assert!(interface.declares_input0 && interface.declares_output);
        assert!(ShaderInterface::parse("@compute @workgroup_size(8, 8) fn cs_main() {}").is_ok());
    }
}
