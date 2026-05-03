//! Operation-agnostic GPU render-plan core for Lumen.
//!
//! This crate deliberately does not model semantic nodes such as blur,
//! transform, text, or composite. Lumen owns those concepts and compiles them
//! into resources, shader programs, and generic passes that this crate can
//! allocate and execute.

mod binding;
mod geometry;
mod id;
mod pass;
mod plan;
mod program;
mod renderer;
mod resource;
mod update;

pub use binding::*;
pub use geometry::*;
pub use id::*;
pub use pass::*;
pub use plan::*;
pub use program::*;
pub use renderer::*;
pub use resource::*;
pub use update::*;
pub use wgpu;

#[cfg(test)]
mod tests {
    use super::*;

    const COPY_SHADER: &str = r#"
@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VsOut {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0)
    );
    let uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0)
    );
    var out: VsOut;
    out.position = vec4<f32>(positions[vertex], 0.0, 1.0);
    out.uv = uvs[vertex];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(input_tex, input_sampler, in.uv);
}
"#;

    #[test]
    fn render_plan_records_generic_shader_passes() {
        let size = Size::new(64, 64);
        let mut plan = RenderPlan::builder();
        let source = plan.texture(
            Some("source".to_string()),
            TextureDesc::sampled(size, wgpu::TextureFormat::Rgba8Unorm),
        );
        let target = plan.texture(
            Some("target".to_string()),
            TextureDesc::render_target(size, wgpu::TextureFormat::Rgba8Unorm),
        );
        let sampler = plan.sampler(
            Some("linear".to_string()),
            wgpu::SamplerDescriptor::default(),
        );
        let program = plan.program(ProgramDesc::Render(RenderProgramDesc {
            label: Some("copy".to_string()),
            shader: COPY_SHADER.to_string(),
            vertex_entry: "vs_main".to_string(),
            fragment_entry: "fs_main".to_string(),
            bind_groups: BindGroupLayoutSpec::single(vec![
                BindingLayoutEntry::texture(0, wgpu::ShaderStages::FRAGMENT),
                BindingLayoutEntry::sampler(1, wgpu::ShaderStages::FRAGMENT),
            ]),
            targets: vec![Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            vertex_buffers: Vec::new(),
            primitive: wgpu::PrimitiveState::default(),
        }));
        plan.render_pass(RenderPassDesc {
            label: Some("copy pass".to_string()),
            owner: Some(NodeKey(42)),
            program,
            targets: vec![RenderTargetRef {
                texture: target,
                load: LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            }],
            bindings: vec![
                Binding::sampled_texture(0, 0, source),
                Binding::sampler(0, 1, sampler),
            ],
            vertex_buffers: Vec::new(),
            index_buffer: None,
            draw: DrawCommand::Draw(Draw {
                vertices: 0..3,
                instances: 0..1,
            }),
            scissor: None,
        });
        let plan = plan.build();

        assert_eq!(plan.textures().len(), 2);
        assert_eq!(plan.programs().len(), 1);
        assert_eq!(plan.passes().len(), 1);
    }

    #[test]
    fn frame_update_records_dynamic_uniform_uploads() {
        let uniforms = BufferId(7);
        let bytes = [1, 2, 3, 4];
        let mut update = FrameUpdate::new();
        update.write_buffer(uniforms, 0, &bytes);

        assert_eq!(update.uploads().len(), 1);
    }
}
