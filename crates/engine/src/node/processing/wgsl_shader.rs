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
//! Every input is bound as premultiplied alpha, converting straight-alpha rasters first, and the
//! output is treated as premultiplied alpha.
//!
//! Uniform fields are read from the struct behind the shader's single `var<uniform>`, laid out
//! with WGSL's uniform rules, and set by name through `uniforms`. Unset fields are zero.

use crate::{
    error::RenderError,
    expr::{ExpressionContext, ExpressionValue},
    node::{
        Deferred, NodeId, NodeParamEvalContext, NodeParams, PortRef, ShaderUniformValue,
        ShaderUniforms,
    },
};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, RasterMetadata, compiler,
};

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

const PREMULTIPLY_SHADER: &str = "
@group(0) @binding(0) var straight: texture_2d<f32>;
@group(0) @binding(2) var premultiplied: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(premultiplied);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let color = textureLoad(straight, vec2<i32>(id.xy), 0);
    textureStore(premultiplied, vec2<i32>(id.xy), vec4<f32>(color.rgb * color.a, color.a));
}
";

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
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "wgsl_shader", name = "WGSL Shader", category = "processing")]
pub struct WgslShader {
    pub id: NodeId,
    #[params]
    pub params: WgslShaderParamsDelegate,
    /// Bound as `input0..inputN` in connection order.
    #[input(name = "source", variadic)]
    pub sources: Vec<PortRef>,
}

impl Default for WgslShader {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: WgslShaderParamsDelegate::default(),
            sources: Vec::new(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LumenGlobals {
    resolution: [f32; 2],
    time: f32,
    frame: u32,
}

impl GpuCompileNode for WgslShader {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }
        let (frame, params) = {
            let expr = ctx.expr_context(self.id, "params");
            let params = self.params.eval(&NodeParamEvalContext {
                node_id: self.id,
                expr: &expr,
            })?;
            (expr.frame as u32, params)
        };
        let inputs = self
            .sources
            .iter()
            .filter(|source| !source.is_empty())
            .map(|source| {
                ctx.compile_port(source)
                    .and_then(|output| output.into_raster(source.id, &source.port))
            })
            .collect::<crate::Result<Vec<_>>>()?;
        let Some(first) = inputs.first().copied() else {
            return Err(shader_error(self.id, frame, "needs at least one input"));
        };
        if inputs.len() > MAX_INPUTS {
            return Err(shader_error(
                self.id,
                frame,
                format!(
                    "has {} inputs; WebGPU allows at most {MAX_INPUTS}",
                    inputs.len()
                ),
            ));
        }

        let user_source = if params.shader.trim().is_empty() {
            DEFAULT_SHADER.to_string()
        } else {
            params.shader
        };
        let interface = ShaderInterface::parse(&user_source)
            .map_err(|details| shader_error(self.id, frame, format!("shader: {details}")))?;
        interface.uniform_bytes(
            self.id,
            &params.uniforms,
            &ctx.expr_context(self.id, "uniforms"),
        )?;
        let shader = interface.assemble(&user_source, inputs.len());

        let owner = lumen_gpu::NodeKey(self.id.0);
        let size = first.domain.storage_size;
        let builder = ctx.builder_mut();
        let output = builder.texture_for(
            owner,
            Some(format!("wgsl-shader:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params_buffer = builder.buffer_for(
            owner,
            Some(format!("wgsl-shader:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(interface.uniform_size as u64),
        );
        let globals_buffer = builder.buffer_for(
            owner,
            Some(format!("wgsl-shader:{}:globals", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<LumenGlobals>() as u64),
        );
        let sampler = builder.sampler(
            Some(format!("wgsl-shader:{}:sampler", self.id.0)),
            lumen_gpu::wgpu::SamplerDescriptor {
                address_mode_u: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                address_mode_v: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                address_mode_w: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                mag_filter: lumen_gpu::wgpu::FilterMode::Linear,
                min_filter: lumen_gpu::wgpu::FilterMode::Linear,
                ..Default::default()
            },
        );

        let input_textures: Vec<_> = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| premultiplied_input(builder, self.id, index, input))
            .collect();
        let stage = lumen_gpu::wgpu::ShaderStages::COMPUTE;
        let mut layout = vec![
            lumen_gpu::BindingLayoutEntry::uniform(1, stage),
            lumen_gpu::BindingLayoutEntry::storage_texture(
                OUTPUT_BINDING,
                stage,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
            ),
            lumen_gpu::BindingLayoutEntry::uniform(3, stage),
            lumen_gpu::BindingLayoutEntry::sampler(4, stage),
        ];
        let mut bindings = vec![
            lumen_gpu::Binding::uniform(0, 1, params_buffer),
            lumen_gpu::Binding::storage_texture(0, OUTPUT_BINDING, output),
            lumen_gpu::Binding::uniform(0, 3, globals_buffer),
            lumen_gpu::Binding::sampler(0, 4, sampler),
        ];
        for (index, texture) in input_textures.into_iter().enumerate() {
            let binding = input_binding(index);
            layout.push(lumen_gpu::BindingLayoutEntry::texture(binding, stage));
            bindings.push(lumen_gpu::Binding::sampled_texture(0, binding, texture));
        }
        let program = builder.program_for(
            owner,
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some(format!("wgsl-shader:{}", self.id.0)),
                shader,
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(layout),
            }),
        );
        builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("wgsl-shader:{}:apply", self.id.0)),
            owner: Some(owner),
            program,
            bindings,
            dispatch: compiler::dispatch_for(size).into(),
        });
        for (slot, buffer) in [params_buffer, globals_buffer].into_iter().enumerate() {
            builder.param(
                lumen_gpu::ParamKey {
                    owner,
                    slot: slot as u32,
                },
                lumen_gpu::ParamTarget::Buffer(buffer),
            );
        }

        ctx.register_compiled_node(CompiledWgslShader {
            node_id: self.id,
            params: self.params.clone(),
            interface,
            params_buffer,
            globals_buffer,
            resolution: [size.width as f32, size.height as f32],
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture: output,
            domain: first.domain,
            metadata: RasterMetadata {
                alpha_mode: AlphaMode::Premultiplied,
                ..first.metadata
            },
        }))
    }
}

/// The texture to bind for `input`, premultiplying a straight-alpha raster in an extra pass.
fn premultiplied_input(
    builder: &mut lumen_gpu::RenderPlanBuilder,
    node_id: NodeId,
    index: usize,
    input: &RasterHandle,
) -> lumen_gpu::TextureId {
    if input.metadata.alpha_mode == AlphaMode::Premultiplied {
        return input.texture;
    }
    let owner = lumen_gpu::NodeKey(node_id.0);
    let size = input.domain.storage_size;
    let texture = builder.texture_for(
        owner,
        Some(format!(
            "wgsl-shader:{}:input{index}:premultiplied",
            node_id.0
        )),
        lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
    );
    let stage = lumen_gpu::wgpu::ShaderStages::COMPUTE;
    let program = builder.program_for(
        owner,
        lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
            label: Some("wgsl-shader-premultiply".to_string()),
            shader: PREMULTIPLY_SHADER.to_string(),
            entry: "cs_main".to_string(),
            bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                lumen_gpu::BindingLayoutEntry::texture(0, stage),
                lumen_gpu::BindingLayoutEntry::storage_texture(
                    OUTPUT_BINDING,
                    stage,
                    lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                    lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                ),
            ]),
        }),
    );
    builder.compute_pass(lumen_gpu::ComputePassDesc {
        label: Some(format!(
            "wgsl-shader:{}:premultiply-input{index}",
            node_id.0
        )),
        owner: Some(owner),
        program,
        bindings: vec![
            lumen_gpu::Binding::sampled_texture(0, 0, input.texture),
            lumen_gpu::Binding::storage_texture(0, OUTPUT_BINDING, texture),
        ],
        dispatch: compiler::dispatch_for(size).into(),
    });
    texture
}

/// `input0` sits at binding 0; later inputs follow the prelude's sampler.
fn input_binding(index: usize) -> u32 {
    if index == 0 { 0 } else { 4 + index as u32 }
}

#[derive(Debug, Clone)]
struct CompiledWgslShader {
    node_id: NodeId,
    params: WgslShaderParamsDelegate,
    interface: ShaderInterface,
    params_buffer: lumen_gpu::BufferId,
    globals_buffer: lumen_gpu::BufferId,
    resolution: [f32; 2],
}

impl GpuCompiledNode for CompiledWgslShader {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let expr = ctx.expr_context(self.node_id, "uniforms");
        let uniforms = self
            .interface
            .uniform_bytes(self.node_id, &params.uniforms, &expr)?;
        bound.write_buffer(self.params_buffer, 0, uniforms);
        let globals = LumenGlobals {
            resolution: self.resolution,
            time: expr.time_seconds() as f32,
            frame: expr.frame as u32,
        };
        bound.write_buffer(self.globals_buffer, 0, bytemuck::bytes_of(&globals));
        Ok(())
    }
}

fn shader_error(
    node_id: NodeId,
    frame: u32,
    details: impl Into<String>,
) -> crate::error::LumenError {
    RenderError::NodeEvaluation {
        frame,
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
                return Err(shader_error(
                    node_id,
                    ctx.frame as u32,
                    format!("uniforms.{name}: {details}"),
                ));
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
    let invalid = |details: String| {
        shader_error(
            node_id,
            ctx.frame as u32,
            format!("uniforms.{}: {details}", field.name),
        )
    };
    let ctx = ExpressionContext {
        path: ctx
            .path
            .as_ref()
            .map(|path| format!("{path}.{}", field.name)),
        ..*ctx
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
        ShaderUniformValue::Expr(expr) => match expr.evaluate(&ctx)? {
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
                Deferred::Expr(expr) => number(expr.evaluate(&ctx)?),
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

/// Integer literals of every `@name(...)` attribute; non-literal arguments are errors.
fn attribute_values<'a>(
    tokens: &'a [&'a str],
    name: &'a str,
) -> impl Iterator<Item = Result<u32, String>> + 'a {
    tokens
        .windows(3)
        .enumerate()
        .filter_map(move |(index, window)| {
            if window != ["@", name, "("] {
                return None;
            }
            let args = &tokens[index + 3..];
            let args = &args[..args.iter().position(|&token| token == ")")?];
            let value = match args {
                [literal] => literal.trim_end_matches(['u', 'i']).parse().ok(),
                _ => None,
            };
            Some(
                value.ok_or_else(|| {
                    format!("`@{name}({})` must use an integer literal", args.concat())
                }),
            )
        })
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
            duration_frames: 1,
            path: None,
            graph: None,
            media: None,
            glyph: None,
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
    }
}
