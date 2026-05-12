use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    BindGroupLayoutSpec, Binding, BindingResource, BufferDesc, BufferId, BufferResource,
    ComputeDispatch, ComputePassDesc, ComputeProgramDesc, CopyTextureDesc, DrawCommand,
    FrameUpdate, LoadOp, PassDesc, Program, ProgramDesc, ProgramId, RenderPassDesc, RenderPlan,
    RenderProgramDesc, SamplerId, TextureDesc, TextureId, TextureResource, Upload,
};

struct RuntimeTexture {
    texture: Arc<wgpu::Texture>,
    view: wgpu::TextureView,
    desc: TextureDesc,
}

struct RuntimeBuffer {
    buffer: wgpu::Buffer,
    desc: BufferDesc,
}

struct RuntimeSampler {
    sampler: wgpu::Sampler,
}

struct RuntimeRenderProgram {
    pipeline: wgpu::RenderPipeline,
    bind_group_layouts: Vec<wgpu::BindGroupLayout>,
}

struct RuntimeComputeProgram {
    pipeline: wgpu::ComputePipeline,
    bind_group_layouts: Vec<wgpu::BindGroupLayout>,
}

enum RuntimeProgram {
    Render(RuntimeRenderProgram),
    Compute(RuntimeComputeProgram),
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    textures: Vec<RuntimeTexture>,
    buffers: Vec<RuntimeBuffer>,
    samplers: Vec<RuntimeSampler>,
    programs: Vec<RuntimeProgram>,
}

impl Renderer {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .context("no compatible wgpu adapter")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .context("create wgpu device")?;
        Ok(Self::from_device(device, queue))
    }

    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            device,
            queue,
            textures: Vec::new(),
            buffers: Vec::new(),
            samplers: Vec::new(),
            programs: Vec::new(),
        }
    }

    pub fn prepare_plan(&mut self, plan: &RenderPlan) -> Result<()> {
        self.textures = plan
            .textures
            .iter()
            .map(|resource| self.create_texture(resource))
            .collect::<Result<Vec<_>>>()?;
        self.buffers = plan
            .buffers
            .iter()
            .map(|resource| self.create_buffer(resource))
            .collect();
        self.samplers = plan
            .samplers
            .iter()
            .map(|resource| RuntimeSampler {
                sampler: self.device.create_sampler(&resource.desc),
            })
            .collect();
        self.programs = plan
            .programs
            .iter()
            .map(|program| self.create_program(program))
            .collect::<Result<Vec<_>>>()?;
        Ok(())
    }

    pub fn texture_view(&self, id: TextureId) -> Option<&wgpu::TextureView> {
        self.textures
            .get(id.0 as usize)
            .map(|texture| &texture.view)
    }

    pub fn texture(&self, id: TextureId) -> Option<&wgpu::Texture> {
        self.textures
            .get(id.0 as usize)
            .map(|texture| texture.texture.as_ref())
    }

    pub fn texture_arc(&self, id: TextureId) -> Option<Arc<wgpu::Texture>> {
        self.textures
            .get(id.0 as usize)
            .map(|texture| Arc::clone(&texture.texture))
    }

    pub fn replace_texture(
        &mut self,
        id: TextureId,
        texture: wgpu::Texture,
        desc: TextureDesc,
    ) -> Result<Arc<wgpu::Texture>> {
        self.replace_texture_arc(id, Arc::new(texture), desc)
    }

    pub fn replace_texture_arc(
        &mut self,
        id: TextureId,
        texture: Arc<wgpu::Texture>,
        desc: TextureDesc,
    ) -> Result<Arc<wgpu::Texture>> {
        let runtime = self
            .textures
            .get_mut(id.0 as usize)
            .ok_or_else(|| anyhow!("unknown texture id {id:?}"))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let old = std::mem::replace(&mut runtime.texture, texture);
        runtime.view = view;
        runtime.desc = desc;
        Ok(old)
    }

    pub fn replace_texture_discard_old(
        &mut self,
        id: TextureId,
        texture: wgpu::Texture,
        desc: TextureDesc,
    ) -> Result<()> {
        let _ = self.replace_texture(id, texture, desc)?;
        Ok(())
    }

    pub fn copy_texture_to_external(
        &self,
        source_id: TextureId,
        destination: &wgpu::Texture,
    ) -> Result<wgpu::SubmissionIndex> {
        let source = self.runtime_texture(source_id)?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen-gpu external texture copy"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: destination,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            source.desc.domain.storage_size.as_extent(),
        );
        Ok(self.queue.submit([encoder.finish()]))
    }

    pub fn copy_texture_to_external_discard_submission(
        &self,
        source_id: TextureId,
        destination: &wgpu::Texture,
    ) -> Result<()> {
        let _ = self.copy_texture_to_external(source_id, destination)?;
        Ok(())
    }

    pub fn buffer(&self, id: BufferId) -> Option<&wgpu::Buffer> {
        self.buffers.get(id.0 as usize).map(|buffer| &buffer.buffer)
    }

    pub fn execute(
        &mut self,
        plan: &RenderPlan,
        update: &FrameUpdate<'_>,
    ) -> Result<wgpu::SubmissionIndex> {
        self.apply_frame_update(plan, update)?;
        self.submit_plan(plan)
    }

    pub fn apply_frame_update(
        &mut self,
        plan: &RenderPlan,
        update: &FrameUpdate<'_>,
    ) -> Result<()> {
        self.validate_prepared(plan)?;
        self.apply_uploads(update)
    }

    pub fn submit_plan(&mut self, plan: &RenderPlan) -> Result<wgpu::SubmissionIndex> {
        self.validate_prepared(plan)?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen-gpu render-plan encoder"),
            });

        for pass in &plan.passes {
            match &pass.desc {
                PassDesc::Render(desc) => self.execute_render_pass(desc, &mut encoder)?,
                PassDesc::Compute(desc) => self.execute_compute_pass(desc, &mut encoder)?,
                PassDesc::CopyTexture(desc) => self.execute_copy_texture(desc, &mut encoder)?,
            }
        }

        Ok(self.queue.submit([encoder.finish()]))
    }

    pub fn execute_discard_submission(
        &mut self,
        plan: &RenderPlan,
        update: &FrameUpdate<'_>,
    ) -> Result<()> {
        let _ = self.execute(plan, update)?;
        Ok(())
    }

    fn create_texture(&self, resource: &TextureResource) -> Result<RuntimeTexture> {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: resource.label.as_deref(),
            size: resource.desc.domain.storage_size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: resource.desc.format,
            usage: resource.desc.usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(RuntimeTexture {
            texture: Arc::new(texture),
            view,
            desc: resource.desc,
        })
    }

    fn create_buffer(&self, resource: &BufferResource) -> RuntimeBuffer {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: resource.label.as_deref(),
            size: resource.desc.size.max(1),
            usage: resource.desc.usage,
            mapped_at_creation: false,
        });
        RuntimeBuffer {
            buffer,
            desc: resource.desc,
        }
    }

    fn create_program(&self, program: &Program) -> Result<RuntimeProgram> {
        match &program.desc {
            ProgramDesc::Render(desc) => {
                self.create_render_program(desc).map(RuntimeProgram::Render)
            }
            ProgramDesc::Compute(desc) => self
                .create_compute_program(desc)
                .map(RuntimeProgram::Compute),
        }
    }

    fn create_render_program(&self, desc: &RenderProgramDesc) -> Result<RuntimeRenderProgram> {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: desc.label.as_deref(),
                source: wgpu::ShaderSource::Wgsl(desc.shader.clone().into()),
            });
        let bind_group_layouts = create_bind_group_layouts(&self.device, &desc.bind_groups);
        let layout_refs: Vec<_> = bind_group_layouts.iter().map(Some).collect();
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: desc.label.as_deref(),
                bind_group_layouts: &layout_refs,
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: desc.label.as_deref(),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(&desc.vertex_entry),
                    compilation_options: Default::default(),
                    buffers: &desc.vertex_buffers,
                },
                primitive: desc.primitive,
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(&desc.fragment_entry),
                    compilation_options: Default::default(),
                    targets: &desc.targets,
                }),
                multiview_mask: None,
                cache: None,
            });
        Ok(RuntimeRenderProgram {
            pipeline,
            bind_group_layouts,
        })
    }

    fn create_compute_program(&self, desc: &ComputeProgramDesc) -> Result<RuntimeComputeProgram> {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: desc.label.as_deref(),
                source: wgpu::ShaderSource::Wgsl(desc.shader.clone().into()),
            });
        let bind_group_layouts = create_bind_group_layouts(&self.device, &desc.bind_groups);
        let layout_refs: Vec<_> = bind_group_layouts.iter().map(Some).collect();
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: desc.label.as_deref(),
                bind_group_layouts: &layout_refs,
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: desc.label.as_deref(),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(&desc.entry),
                compilation_options: Default::default(),
                cache: None,
            });
        Ok(RuntimeComputeProgram {
            pipeline,
            bind_group_layouts,
        })
    }

    fn validate_prepared(&self, plan: &RenderPlan) -> Result<()> {
        if self.textures.len() != plan.textures.len()
            || self.buffers.len() != plan.buffers.len()
            || self.samplers.len() != plan.samplers.len()
            || self.programs.len() != plan.programs.len()
        {
            bail!("render plan has not been prepared or has changed since prepare_plan");
        }
        Ok(())
    }

    fn apply_uploads(&self, update: &FrameUpdate<'_>) -> Result<()> {
        for upload in update.uploads() {
            match upload {
                Upload::Buffer { id, offset, data } => {
                    let buffer = self.runtime_buffer(*id)?;
                    let end = offset.saturating_add(data.len() as u64);
                    if end > buffer.desc.size {
                        bail!("buffer upload for {id:?} exceeds declared buffer size");
                    }
                    self.queue.write_buffer(&buffer.buffer, *offset, data);
                }
                Upload::TextureRgba8 {
                    id,
                    data,
                    bytes_per_row,
                    rows_per_image,
                } => {
                    let texture = self.runtime_texture(*id)?;
                    self.queue.write_texture(
                        texture.texture.as_image_copy(),
                        data,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(*bytes_per_row),
                            rows_per_image: Some(*rows_per_image),
                        },
                        texture.desc.domain.storage_size.as_extent(),
                    );
                }
                Upload::TextureRgba8Region {
                    id,
                    data,
                    origin,
                    size,
                    bytes_per_row,
                    rows_per_image,
                } => {
                    let texture = self.runtime_texture(*id)?;
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: origin[0],
                                y: origin[1],
                                z: origin[2],
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        data,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(*bytes_per_row),
                            rows_per_image: Some(*rows_per_image),
                        },
                        size.as_extent(),
                    );
                }
                Upload::TextureRgba16Float {
                    id,
                    data,
                    bytes_per_row,
                    rows_per_image,
                } => {
                    let texture = self.runtime_texture(*id)?;
                    self.queue.write_texture(
                        texture.texture.as_image_copy(),
                        bytemuck::cast_slice(data),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(*bytes_per_row),
                            rows_per_image: Some(*rows_per_image),
                        },
                        texture.desc.domain.storage_size.as_extent(),
                    );
                }
            }
        }
        Ok(())
    }

    fn execute_render_pass(
        &self,
        desc: &RenderPassDesc,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<()> {
        let RuntimeProgram::Render(program) = self.runtime_program(desc.program)? else {
            bail!("program {:?} is not a render program", desc.program);
        };
        let attachments = desc
            .targets
            .iter()
            .map(|target| {
                let texture = self.runtime_texture(target.texture)?;
                Ok(Some(wgpu::RenderPassColorAttachment {
                    view: &texture.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: match target.load {
                            LoadOp::Load => wgpu::LoadOp::Load,
                            LoadOp::Clear(color) => wgpu::LoadOp::Clear(color),
                        },
                        store: target.store,
                    },
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: desc.label.as_deref(),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&program.pipeline);
        let bind_groups =
            self.create_pass_bind_groups(&program.bind_group_layouts, &desc.bindings)?;
        for (group, bind_group) in bind_groups.iter().enumerate() {
            pass.set_bind_group(group as u32, bind_group, &[]);
        }
        for (slot, buffer_id) in desc.vertex_buffers.iter().enumerate() {
            pass.set_vertex_buffer(
                slot as u32,
                self.runtime_buffer(*buffer_id)?.buffer.slice(..),
            );
        }
        if let Some((buffer_id, format)) = desc.index_buffer {
            pass.set_index_buffer(self.runtime_buffer(buffer_id)?.buffer.slice(..), format);
        }
        if let Some(scissor) = desc.scissor {
            pass.set_scissor_rect(scissor.x, scissor.y, scissor.width, scissor.height);
        }
        match &desc.draw {
            DrawCommand::Draw(draw) => pass.draw(draw.vertices.clone(), draw.instances.clone()),
            DrawCommand::DrawIndexed(draw) => pass.draw_indexed(
                draw.indices.clone(),
                draw.base_vertex,
                draw.instances.clone(),
            ),
        }
        Ok(())
    }

    fn execute_compute_pass(
        &self,
        desc: &ComputePassDesc,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<()> {
        let RuntimeProgram::Compute(program) = self.runtime_program(desc.program)? else {
            bail!("program {:?} is not a compute program", desc.program);
        };
        let bind_groups =
            self.create_pass_bind_groups(&program.bind_group_layouts, &desc.bindings)?;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: desc.label.as_deref(),
            timestamp_writes: None,
        });
        pass.set_pipeline(&program.pipeline);
        for (group, bind_group) in bind_groups.iter().enumerate() {
            pass.set_bind_group(group as u32, bind_group, &[]);
        }
        match desc.dispatch {
            ComputeDispatch::Direct(dispatch) => {
                pass.dispatch_workgroups(dispatch.x, dispatch.y, dispatch.z);
            }
            ComputeDispatch::Indirect { buffer, offset } => {
                let buffer = &self.runtime_buffer(buffer)?.buffer;
                pass.dispatch_workgroups_indirect(buffer, offset);
            }
        }
        Ok(())
    }

    fn execute_copy_texture(
        &self,
        desc: &CopyTextureDesc,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<()> {
        let source = self.runtime_texture(desc.source)?;
        let destination = self.runtime_texture(desc.destination)?;
        encoder.copy_texture_to_texture(
            source.texture.as_image_copy(),
            wgpu::TexelCopyTextureInfo {
                texture: &destination.texture,
                mip_level: 0,
                origin: desc.origin,
                aspect: wgpu::TextureAspect::All,
            },
            desc.size.as_extent(),
        );
        Ok(())
    }

    fn create_pass_bind_groups(
        &self,
        layouts: &[wgpu::BindGroupLayout],
        bindings: &[Binding],
    ) -> Result<Vec<wgpu::BindGroup>> {
        let mut grouped: HashMap<u32, Vec<&Binding>> = HashMap::new();
        for binding in bindings {
            grouped.entry(binding.group).or_default().push(binding);
        }
        let mut bind_groups = Vec::with_capacity(layouts.len());
        for (group_index, layout) in layouts.iter().enumerate() {
            let mut group_entries = grouped.remove(&(group_index as u32)).unwrap_or_default();
            group_entries.sort_by_key(|entry| entry.binding);
            let entries = group_entries
                .iter()
                .map(|binding| self.bind_group_entry(binding))
                .collect::<Result<Vec<_>>>()?;
            bind_groups.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &entries,
            }));
        }
        if !grouped.is_empty() {
            bail!("pass contains bindings for groups not declared by its program");
        }
        Ok(bind_groups)
    }

    fn bind_group_entry(&self, binding: &Binding) -> Result<wgpu::BindGroupEntry<'_>> {
        let resource = match binding.resource {
            BindingResource::Texture { id, .. } => {
                wgpu::BindingResource::TextureView(&self.runtime_texture(id)?.view)
            }
            BindingResource::Buffer { id, .. } => {
                self.runtime_buffer(id)?.buffer.as_entire_binding()
            }
            BindingResource::Sampler(id) => {
                wgpu::BindingResource::Sampler(&self.runtime_sampler(id)?.sampler)
            }
        };
        Ok(wgpu::BindGroupEntry {
            binding: binding.binding,
            resource,
        })
    }

    fn runtime_texture(&self, id: TextureId) -> Result<&RuntimeTexture> {
        self.textures
            .get(id.0 as usize)
            .ok_or_else(|| anyhow!("unknown texture id {id:?}"))
    }

    fn runtime_buffer(&self, id: BufferId) -> Result<&RuntimeBuffer> {
        self.buffers
            .get(id.0 as usize)
            .ok_or_else(|| anyhow!("unknown buffer id {id:?}"))
    }

    fn runtime_sampler(&self, id: SamplerId) -> Result<&RuntimeSampler> {
        self.samplers
            .get(id.0 as usize)
            .ok_or_else(|| anyhow!("unknown sampler id {id:?}"))
    }

    fn runtime_program(&self, id: ProgramId) -> Result<&RuntimeProgram> {
        self.programs
            .get(id.0 as usize)
            .ok_or_else(|| anyhow!("unknown program id {id:?}"))
    }
}

fn create_bind_group_layouts(
    device: &wgpu::Device,
    specs: &[BindGroupLayoutSpec],
) -> Vec<wgpu::BindGroupLayout> {
    specs
        .iter()
        .map(|spec| {
            let entries: Vec<_> = spec
                .entries
                .iter()
                .map(|entry| wgpu::BindGroupLayoutEntry {
                    binding: entry.binding,
                    visibility: entry.visibility,
                    ty: entry.ty,
                    count: None,
                })
                .collect();
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: spec.label.as_deref(),
                entries: &entries,
            })
        })
        .collect()
}
