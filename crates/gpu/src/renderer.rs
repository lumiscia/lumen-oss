use std::{collections::HashMap, env, sync::Arc};

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
    adapter_info: GpuAdapterInfo,
    textures: Vec<RuntimeTexture>,
    buffers: Vec<RuntimeBuffer>,
    samplers: Vec<RuntimeSampler>,
    programs: Vec<RuntimeProgram>,
}

#[derive(Debug, Clone)]
pub struct GpuPassTiming {
    pub label: String,
    pub kind: &'static str,
    pub nanoseconds: f64,
}

#[derive(Debug, Clone, Default)]
pub struct GpuPlanTiming {
    pub passes: Vec<GpuPassTiming>,
}

impl GpuPlanTiming {
    pub fn total_nanoseconds(&self) -> f64 {
        self.passes.iter().map(|pass| pass.nanoseconds).sum()
    }
}

#[derive(Clone)]
pub struct ExternalTexture {
    texture: Arc<wgpu::Texture>,
    desc: TextureDesc,
}

impl std::fmt::Debug for ExternalTexture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalTexture")
            .field("desc", &self.desc)
            .finish_non_exhaustive()
    }
}

impl ExternalTexture {
    pub fn new(texture: Arc<wgpu::Texture>, desc: TextureDesc) -> Self {
        Self { texture, desc }
    }

    pub fn from_texture(texture: wgpu::Texture, desc: TextureDesc) -> Self {
        Self::new(Arc::new(texture), desc)
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn texture_arc(&self) -> Arc<wgpu::Texture> {
        Arc::clone(&self.texture)
    }

    pub fn desc(&self) -> TextureDesc {
        self.desc
    }
}

#[derive(Debug, Clone)]
pub struct SubmittedExternalTexture {
    submission: wgpu::SubmissionIndex,
    texture: Arc<wgpu::Texture>,
}

impl SubmittedExternalTexture {
    pub fn submission(&self) -> wgpu::SubmissionIndex {
        self.submission.clone()
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn wait(&self, device: &wgpu::Device) -> Result<()> {
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.submission.clone()),
                timeout: None,
            })
            .context("wait for external texture submission")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuAdapterInfo {
    pub name: String,
    pub backend: wgpu::Backend,
    pub device_type: wgpu::DeviceType,
    pub vendor: u32,
    pub device: u32,
    pub driver: String,
    pub driver_info: String,
}

impl From<wgpu::AdapterInfo> for GpuAdapterInfo {
    fn from(info: wgpu::AdapterInfo) -> Self {
        Self {
            name: info.name,
            backend: info.backend,
            device_type: info.device_type,
            vendor: info.vendor,
            device: info.device,
            driver: info.driver,
            driver_info: info.driver_info,
        }
    }
}

impl Renderer {
    pub async fn new() -> Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let power_preference =
            wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance);
        let force_fallback_adapter = env_flag("LUMEN_GPU_FORCE_FALLBACK_ADAPTER");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                compatible_surface: None,
                force_fallback_adapter,
            })
            .await
            .context("no compatible wgpu adapter")?;
        let adapter_info = GpuAdapterInfo::from(adapter.get_info());
        tracing::info!(
            target: "lumen_gpu",
            adapter = %adapter_info.name,
            backend = ?adapter_info.backend,
            device_type = ?adapter_info.device_type,
            vendor = adapter_info.vendor,
            device = adapter_info.device,
            driver = %adapter_info.driver,
            driver_info = %adapter_info.driver_info,
            "selected wgpu adapter"
        );
        let device_descriptor = wgpu::DeviceDescriptor {
            required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
            ..Default::default()
        };
        let (device, queue) = adapter
            .request_device(&device_descriptor)
            .await
            .context("create wgpu device")?;
        Ok(Self::from_device_with_adapter_info(
            device,
            queue,
            adapter_info,
        ))
    }

    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self::from_device_with_adapter_info(
            device,
            queue,
            GpuAdapterInfo {
                name: "external device".to_string(),
                backend: wgpu::Backend::Noop,
                device_type: wgpu::DeviceType::Other,
                vendor: 0,
                device: 0,
                driver: "external".to_string(),
                driver_info: "created outside lumen-gpu".to_string(),
            },
        )
    }

    pub fn from_device_with_adapter_info(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter_info: GpuAdapterInfo,
    ) -> Self {
        Self {
            device,
            queue,
            adapter_info,
            textures: Vec::new(),
            buffers: Vec::new(),
            samplers: Vec::new(),
            programs: Vec::new(),
        }
    }

    pub fn adapter_info(&self) -> &GpuAdapterInfo {
        &self.adapter_info
    }

    pub fn supports_timestamp_queries(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
    }

    pub fn prepare_plan(&mut self, plan: &RenderPlan) -> Result<()> {
        tracing::debug!(
            target: "lumen_gpu",
            textures = plan.textures.len(),
            buffers = plan.buffers.len(),
            samplers = plan.samplers.len(),
            programs = plan.programs.len(),
            passes = plan.passes.len(),
            "prepare render plan"
        );
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

    pub fn texture_desc(&self, id: TextureId) -> Option<TextureDesc> {
        self.textures.get(id.0 as usize).map(|texture| texture.desc)
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

    pub fn submit_plan_with_external_texture(
        &mut self,
        plan: &RenderPlan,
        texture_id: TextureId,
        external: ExternalTexture,
    ) -> Result<SubmittedExternalTexture> {
        self.validate_external_texture(texture_id, external.desc())?;
        let old_desc = self
            .texture_desc(texture_id)
            .ok_or_else(|| anyhow!("unknown texture id {texture_id:?}"))?;
        let retained = external.texture_arc();
        let old_texture =
            self.replace_texture_arc(texture_id, retained.clone(), external.desc())?;
        let submission = match self.submit_plan(plan) {
            Ok(submission) => submission,
            Err(error) => {
                let _ = self.replace_texture_arc(texture_id, old_texture, old_desc);
                return Err(error);
            }
        };
        let _ = self.replace_texture_arc(texture_id, old_texture, old_desc)?;
        Ok(SubmittedExternalTexture {
            submission,
            texture: retained,
        })
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
        tracing::trace!(
            target: "lumen_gpu",
            uploads = update.uploads().len(),
            "apply frame update"
        );
        self.apply_uploads(update)
    }

    pub fn submit_plan(&mut self, plan: &RenderPlan) -> Result<wgpu::SubmissionIndex> {
        self.validate_prepared(plan)?;
        tracing::trace!(
            target: "lumen_gpu",
            passes = plan.passes.len(),
            "submit render plan"
        );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen-gpu render-plan encoder"),
            });

        for pass in &plan.passes {
            match &pass.desc {
                PassDesc::Render(desc) => self.execute_render_pass(desc, &mut encoder, None)?,
                PassDesc::Compute(desc) => self.execute_compute_pass(desc, &mut encoder, None)?,
                PassDesc::CopyTexture(desc) => self.execute_copy_texture(desc, &mut encoder)?,
            }
        }

        Ok(self.queue.submit([encoder.finish()]))
    }

    pub fn submit_plan_profiled(&mut self, plan: &RenderPlan) -> Result<Option<GpuPlanTiming>> {
        self.validate_prepared(plan)?;
        if !self.supports_timestamp_queries() {
            return Ok(None);
        }
        let timed_passes = plan
            .passes
            .iter()
            .filter(|pass| matches!(pass.desc, PassDesc::Render(_) | PassDesc::Compute(_)))
            .count();
        if timed_passes == 0 {
            return Ok(Some(GpuPlanTiming::default()));
        }
        let query_count = (timed_passes * 2) as u32;
        let query_set = self.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("lumen-gpu plan timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: query_count,
        });
        let query_bytes = u64::from(query_count) * std::mem::size_of::<u64>() as u64;
        let resolve = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen-gpu timestamp resolve"),
            size: query_bytes,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen-gpu timestamp readback"),
            size: query_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen-gpu profiled render-plan encoder"),
            });
        let mut metadata = Vec::with_capacity(timed_passes);
        let mut timed_index = 0_u32;
        for pass in &plan.passes {
            match &pass.desc {
                PassDesc::Render(desc) => {
                    let start = timed_index * 2;
                    self.execute_render_pass(desc, &mut encoder, Some((&query_set, start)))?;
                    metadata.push((
                        desc.label.clone().unwrap_or_else(|| "render".to_string()),
                        "render",
                    ));
                    timed_index += 1;
                }
                PassDesc::Compute(desc) => {
                    let start = timed_index * 2;
                    self.execute_compute_pass(desc, &mut encoder, Some((&query_set, start)))?;
                    metadata.push((
                        desc.label.clone().unwrap_or_else(|| "compute".to_string()),
                        "compute",
                    ));
                    timed_index += 1;
                }
                PassDesc::CopyTexture(desc) => self.execute_copy_texture(desc, &mut encoder)?,
            }
        }
        encoder.resolve_query_set(&query_set, 0..query_count, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &readback, 0, query_bytes);
        let submission = self.queue.submit([encoder.finish()]);
        self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })?;

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        receiver
            .recv()
            .context("receive GPU timestamp mapping result")?
            .context("map GPU timestamp readback")?;
        let mapped = slice.get_mapped_range();
        let timestamps = bytemuck::cast_slice::<u8, u64>(&mapped);
        let period = f64::from(self.queue.get_timestamp_period());
        let passes = metadata
            .into_iter()
            .enumerate()
            .map(|(index, (label, kind))| GpuPassTiming {
                label,
                kind,
                nanoseconds: timestamps[index * 2 + 1].saturating_sub(timestamps[index * 2]) as f64
                    * period,
            })
            .collect();
        drop(mapped);
        readback.unmap();
        Ok(Some(GpuPlanTiming { passes }))
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

    fn validate_external_texture(&self, id: TextureId, external: TextureDesc) -> Result<()> {
        let slot = self
            .textures
            .get(id.0 as usize)
            .ok_or_else(|| anyhow!("unknown texture id {id:?}"))?;
        let expected = slot.desc;
        if expected.domain.storage_size != external.domain.storage_size {
            bail!(
                "external texture for {id:?} has size {:?}, expected {:?}",
                external.domain.storage_size,
                expected.domain.storage_size
            );
        }
        if expected.format != external.format {
            bail!(
                "external texture for {id:?} has format {:?}, expected {:?}",
                external.format,
                expected.format
            );
        }
        if !external.usage.contains(expected.usage) {
            bail!(
                "external texture for {id:?} has usage {:?}, expected at least {:?}",
                external.usage,
                expected.usage
            );
        }
        Ok(())
    }

    fn apply_uploads(&self, update: &FrameUpdate<'_>) -> Result<()> {
        for upload in update.uploads() {
            match upload {
                Upload::Buffer { id, offset, data } => {
                    tracing::trace!(
                        target: "lumen_gpu",
                        ?id,
                        offset,
                        bytes = data.len(),
                        "upload buffer"
                    );
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
                    tracing::trace!(
                        target: "lumen_gpu",
                        ?id,
                        bytes = data.len(),
                        bytes_per_row,
                        rows_per_image,
                        "upload rgba8 texture"
                    );
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
                    tracing::trace!(
                        target: "lumen_gpu",
                        ?id,
                        bytes = data.len(),
                        origin_x = origin[0],
                        origin_y = origin[1],
                        origin_z = origin[2],
                        width = size.width,
                        height = size.height,
                        "upload rgba8 texture region"
                    );
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
                    tracing::trace!(
                        target: "lumen_gpu",
                        ?id,
                        bytes = std::mem::size_of_val(*data),
                        bytes_per_row,
                        rows_per_image,
                        "upload rgba16f texture"
                    );
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
                Upload::TextureRgba16FloatRegion {
                    id,
                    data,
                    origin,
                    size,
                    bytes_per_row,
                    rows_per_image,
                } => {
                    tracing::trace!(
                        target: "lumen_gpu",
                        ?id,
                        bytes = std::mem::size_of_val(*data),
                        origin_x = origin[0],
                        origin_y = origin[1],
                        origin_z = origin[2],
                        width = size.width,
                        height = size.height,
                        "upload rgba16f texture region"
                    );
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
                        bytemuck::cast_slice(data),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(*bytes_per_row),
                            rows_per_image: Some(*rows_per_image),
                        },
                        size.as_extent(),
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
        timestamps: Option<(&wgpu::QuerySet, u32)>,
    ) -> Result<()> {
        tracing::trace!(
            target: "lumen_gpu",
            label = desc.label.as_deref().unwrap_or(""),
            targets = desc.targets.len(),
            bind_groups = desc.bindings.len(),
            vertex_buffers = desc.vertex_buffers.len(),
            "encode render pass"
        );
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
        let timestamp_writes =
            timestamps.map(|(query_set, start)| wgpu::RenderPassTimestampWrites {
                query_set,
                beginning_of_pass_write_index: Some(start),
                end_of_pass_write_index: Some(start + 1),
            });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: desc.label.as_deref(),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes,
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
        timestamps: Option<(&wgpu::QuerySet, u32)>,
    ) -> Result<()> {
        match desc.dispatch {
            ComputeDispatch::Direct(dispatch) => tracing::trace!(
                target: "lumen_gpu",
                label = desc.label.as_deref().unwrap_or(""),
                x = dispatch.x,
                y = dispatch.y,
                z = dispatch.z,
                "encode compute pass"
            ),
            ComputeDispatch::Indirect { buffer, offset } => tracing::trace!(
                target: "lumen_gpu",
                label = desc.label.as_deref().unwrap_or(""),
                ?buffer,
                offset,
                "encode indirect compute pass"
            ),
        }
        let RuntimeProgram::Compute(program) = self.runtime_program(desc.program)? else {
            bail!("program {:?} is not a compute program", desc.program);
        };
        let bind_groups =
            self.create_pass_bind_groups(&program.bind_group_layouts, &desc.bindings)?;
        let timestamp_writes =
            timestamps.map(|(query_set, start)| wgpu::ComputePassTimestampWrites {
                query_set,
                beginning_of_pass_write_index: Some(start),
                end_of_pass_write_index: Some(start + 1),
            });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: desc.label.as_deref(),
            timestamp_writes,
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
        tracing::trace!(
            target: "lumen_gpu",
            source = ?desc.source,
            destination = ?desc.destination,
            width = desc.size.width,
            height = desc.size.height,
            "encode texture copy"
        );
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

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
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
