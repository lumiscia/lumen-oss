use crate::{
    expr::Expression,
    node::{NodeId, NodeParamEvalContext, NodeParams, PortRef},
};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, compiler,
};

const PREMULTIPLIED_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    "const INPUT_PREMULTIPLIED: bool = true;\n",
    include_str!("wgsl_shader.wgsl")
);
const STRAIGHT_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    "const INPUT_PREMULTIPLIED: bool = false;\n",
    include_str!("wgsl_shader.wgsl")
);

// TODO: replace the stringly shader/bindings surface with typed shader
// parameters once custom-node authoring settles.
/// Runs a custom WGSL compute shader over a raster.
#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct WgslShaderParams {
    /// Custom WGSL compute shader source.
    #[meta(
        name = "Shader source",
        format = "wgsl",
        multiline,
        recommended_rows = 10
    )]
    pub shader: String,
    /// JSON object describing shader binding values.
    #[meta(
        name = "Shader bindings",
        format = "json",
        multiline,
        recommended_rows = 6
    )]
    pub bindings: String,
}

/// Runs a custom WGSL compute shader over a raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "wgsl_shader", name = "WGSL Shader", category = "processing")]
pub struct WgslShader {
    pub id: NodeId,
    #[params]
    pub params: WgslShaderParamsDelegate,
    #[input()]
    pub source: PortRef,
}

impl Default for WgslShader {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: WgslShaderParamsDelegate::default(),
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
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.id,
            expr: &ctx.expr_context(self.id, "params"),
        })?;
        let shader = if params.shader.trim().is_empty() {
            let source = ctx
                .compile_port(&self.source)?
                .into_raster(self.source.id, &self.source.port)?;
            match source.metadata.alpha_mode {
                AlphaMode::Premultiplied => PREMULTIPLIED_SHADER,
                AlphaMode::Unpremultiplied => STRAIGHT_SHADER,
            }
        } else {
            params.shader.as_str()
        };
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "wgsl-shader",
            shader,
            std::mem::size_of::<compiler::WgslShaderParams>() as u64,
            None,
        )?;
        ctx.register_compiled_node(CompiledWgslShader {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledWgslShader {
    node_id: NodeId,
    params: WgslShaderParamsDelegate,
    buffer: lumen_gpu::BufferId,
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
        let params = compiler::WgslShaderParams {
            values: resolve_shader_values(
                self.node_id,
                &ctx.expr_context(self.node_id, "bindings"),
                &params.shader,
                &params.bindings,
            )?,
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
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
            .map(|value| value as f32)
            .filter(|value| value.is_finite())
            .ok_or_else(|| crate::error::PropertyError::InvalidType {
                node_id,
                property_path: format!("bindings.{name}"),
                expected: "Float",
                actual: "Expression",
            })?;
        return Ok(vec![value]);
    }

    let parts: Vec<&str> = if raw_value.contains(',') {
        raw_value.split(',').map(str::trim).collect()
    } else {
        // An empty value yields one empty part so it is rejected rather than binding nothing.
        raw_value
            .split_whitespace()
            .chain(raw_value.is_empty().then_some(""))
            .collect()
    };
    parts
        .into_iter()
        .map(|part| {
            part.parse::<f32>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    crate::error::PropertyError::InvalidType {
                        node_id,
                        property_path: format!("bindings.{name}"),
                        expected: "Float",
                        actual: "String",
                    }
                    .into()
                })
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> crate::expr::ExpressionContext<'static> {
        crate::expr::ExpressionContext {
            frame: 0,
            fps: 30.0,
            width: 1,
            height: 1,
            duration_frames: 1,
            path: None,
            graph: None,
            media: None,
        }
    }

    #[test]
    fn binding_components_must_be_numbers() {
        let node_id = NodeId::new(7);
        let ctx = context();
        assert_eq!(
            parse_binding_values(node_id, "tint", "1, 0.5 ,0", &ctx).unwrap(),
            vec![1.0, 0.5, 0.0]
        );
        assert_eq!(
            parse_binding_values(node_id, "tint", "1\t0.5  0", &ctx).unwrap(),
            vec![1.0, 0.5, 0.0]
        );
        for raw in [
            "1, abc",
            "0.5 oops",
            "1,,0.5,1",
            ",,,",
            "1,",
            "",
            "NaN",
            "inf",
            "1e999,0",
            "=pow(10, 40)",
            "=pow(-1, 0.5)",
        ] {
            let error = parse_binding_values(node_id, "tint", raw, &ctx).unwrap_err();
            assert!(
                error.to_string().contains("bindings.tint"),
                "{raw}: {error}"
            );
        }
    }
}
