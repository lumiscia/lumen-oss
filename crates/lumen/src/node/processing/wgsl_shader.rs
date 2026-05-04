use crate::{
    expr::Expression,
    node::{NodeId, NodeProperty, PortRef},
};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("wgsl_shader.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "wgsl_shader",
    label = "WGSL Shader",
    description = "Runs a custom WGSL compute shader over a raster.",
    category = "processing"
)]
pub struct WgslShader {
    pub id: NodeId,
    #[property(kind = "string")]
    pub shader: NodeProperty,
    #[property(kind = "string")]
    pub bindings: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for WgslShader {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            shader: NodeProperty::String(String::new()),
            bindings: NodeProperty::String(String::new()),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for WgslShader {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let shader =
            self.shader
                .resolve_string(self.id, "shader", &ctx.expr_context(self.id, "shader"))?;
        let shader = if shader.trim().is_empty() {
            SHADER
        } else {
            shader.as_str()
        };
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "wgsl-shader",
            shader,
            std::mem::size_of::<compiler::WgslShaderParams>() as u64,
        )?;
        ctx.push_frame_binding(FrameBinding::WgslShader {
            node_id: self.id,
            shader: self.shader.clone(),
            bindings: self.bindings.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for WgslShader {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::WgslShader {
            node_id,
            shader,
            bindings,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let shader =
            shader.resolve_string(*node_id, "shader", &ctx.expr_context(*node_id, "shader"))?;
        let bindings = bindings.resolve_string(
            *node_id,
            "bindings",
            &ctx.expr_context(*node_id, "bindings"),
        )?;
        let params = compiler::WgslShaderParams {
            values: resolve_shader_values(
                *node_id,
                &ctx.expr_context(*node_id, "bindings"),
                &shader,
                &bindings,
            )?,
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

fn resolve_shader_values(
    node_id: NodeId,
    ctx: &crate::expr::ExpressionContext<'_>,
    shader: &str,
    bindings: &str,
) -> crate::Result<[f32; 4]> {
    let fields = uniform_field_names(shader);
    let mut values = [0.0; 4];

    for raw_line in bindings.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((raw_name, raw_value)) = line.split_once('=') else {
            continue;
        };
        let name = raw_name.trim();
        let raw_value = raw_value.trim();
        if name.is_empty() || raw_value.starts_with("input") {
            continue;
        }

        let Some(offset) = field_offset(&fields, name) else {
            continue;
        };
        for (index, component) in parse_binding_values(node_id, name, raw_value, ctx)?
            .into_iter()
            .take(4 - offset)
            .enumerate()
        {
            values[offset + index] = component;
        }
    }

    Ok(values)
}

fn parse_binding_values(
    node_id: NodeId,
    name: &str,
    raw_value: &str,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<Vec<f32>> {
    let raw_value = raw_value.trim();
    if let Some(expression) = raw_value.strip_prefix('=') {
        let value = Expression::parse(expression)
            .map_err(crate::error::LumenError::from)?
            .evaluate(ctx)?
            .as_f64()
            .ok_or_else(|| crate::error::PropertyError::InvalidType {
                node_id,
                property_path: format!("bindings.{name}"),
                expected: "Float",
                actual: "Expression",
            })?;
        return Ok(vec![value as f32]);
    }

    let delimiter: &[_] = if raw_value.contains(',') {
        &[',']
    } else {
        &[' ', '\t']
    };
    Ok(raw_value
        .split(delimiter)
        .filter(|part| !part.trim().is_empty())
        .filter_map(|part| part.trim().parse::<f32>().ok())
        .collect())
}

fn field_offset(fields: &[UniformField], name: &str) -> Option<usize> {
    let mut offset = 0;
    for field in fields {
        if field.name == name {
            return Some(offset);
        }
        offset += field.lanes;
        if offset >= 4 {
            break;
        }
    }
    None
}

#[derive(Debug, Clone)]
struct UniformField {
    name: String,
    lanes: usize,
}

fn uniform_field_names(shader: &str) -> Vec<UniformField> {
    let struct_name = shader
        .split("var<uniform>")
        .nth(1)
        .and_then(|tail| tail.split(';').next())
        .and_then(|decl| decl.split(':').nth(1))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("ShaderParams");
    let Some(struct_body) = extract_struct_body(shader, struct_name) else {
        return vec![UniformField {
            name: "values".to_string(),
            lanes: 4,
        }];
    };

    struct_body
        .split([',', '\n'])
        .filter_map(|line| {
            let line = line.trim();
            let (name, ty) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(UniformField {
                name: name.to_string(),
                lanes: lanes_for_wgsl_type(ty.trim()),
            })
        })
        .collect()
}

fn extract_struct_body<'a>(shader: &'a str, struct_name: &str) -> Option<&'a str> {
    let start = shader.find(&format!("struct {struct_name}"))?;
    let body_start = shader[start..].find('{')? + start + 1;
    let body_end = shader[body_start..].find('}')? + body_start;
    Some(&shader[body_start..body_end])
}

fn lanes_for_wgsl_type(ty: &str) -> usize {
    if ty.contains("vec4") {
        4
    } else if ty.contains("vec3") {
        3
    } else if ty.contains("vec2") {
        2
    } else {
        1
    }
}
