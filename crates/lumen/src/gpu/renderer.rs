use std::{collections::HashMap, sync::Arc};

use crate::{
    composition::Composition,
    error::RenderError,
    gpu::{
        BoundFrame, CompileContext, CompiledComposition, FrameBindContext, MediaTextureKey,
        RasterHandle,
    },
    media::{CpuMediaFrame, MediaStore},
    node::{NodeId, NodeKind, PortRef},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompiledPlanKey(Vec<(NodeId, Option<usize>)>);

struct PreparedComposition {
    renderer: lumen_gpu::Renderer,
    compiled: CompiledComposition,
    current_media_textures: HashMap<lumen_gpu::TextureId, MediaTextureKey>,
}

pub struct GpuCompositionRenderer {
    renderer: lumen_gpu::Renderer,
    compiled_plans: HashMap<CompiledPlanKey, PreparedComposition>,
    active_key: Option<CompiledPlanKey>,
    output_format: lumen_gpu::wgpu::TextureFormat,
    media_texture_cache: HashMap<MediaTextureKey, Arc<lumen_gpu::wgpu::Texture>>,
}

impl GpuCompositionRenderer {
    pub async fn new() -> crate::Result<Self> {
        let renderer = lumen_gpu::Renderer::new()
            .await
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        Ok(Self {
            renderer,
            compiled_plans: HashMap::new(),
            active_key: None,
            output_format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            media_texture_cache: HashMap::new(),
        })
    }

    pub fn from_device(device: lumen_gpu::wgpu::Device, queue: lumen_gpu::wgpu::Queue) -> Self {
        Self {
            renderer: lumen_gpu::Renderer::from_device(device, queue),
            compiled_plans: HashMap::new(),
            active_key: None,
            output_format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            media_texture_cache: HashMap::new(),
        }
    }

    pub fn compile(&mut self, composition: &Composition) -> crate::Result<()> {
        self.compile_with_output_format(composition, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
    }

    pub fn compile_with_output_format(
        &mut self,
        composition: &Composition,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> crate::Result<()> {
        self.reset_compiled(output_format);
        let key = self.compiled_plan_key(composition, 0)?;
        let compiled = CompileContext::with_frame(composition, 0, output_format).compile()?;
        self.prepare_compiled(key, compiled)
    }

    pub fn compile_with_media<M: MediaStore>(
        &mut self,
        composition: &Composition,
        media: &M,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> crate::Result<()> {
        self.reset_compiled(output_format);
        self.ensure_compiled_for_frame(composition, 0, Some(media))
    }

    fn reset_compiled(&mut self, output_format: lumen_gpu::wgpu::TextureFormat) {
        self.output_format = output_format;
        self.compiled_plans.clear();
        self.active_key = None;
    }

    fn prepare_compiled(
        &mut self,
        key: CompiledPlanKey,
        compiled: CompiledComposition,
    ) -> crate::Result<()> {
        let mut renderer = lumen_gpu::Renderer::from_device(
            self.renderer.device.clone(),
            self.renderer.queue.clone(),
        );
        renderer
            .prepare_plan(&compiled.plan)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        self.compiled_plans.insert(
            key.clone(),
            PreparedComposition {
                renderer,
                compiled,
                current_media_textures: HashMap::new(),
            },
        );
        self.active_key = Some(key);
        Ok(())
    }

    pub fn precompile_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<()> {
        self.ensure_compiled_for_frame(composition, frame, Some(media))
    }

    pub fn precompile_frame_window<M: MediaStore>(
        &mut self,
        composition: &Composition,
        start_frame: u32,
        frame_count: u32,
        media: &M,
    ) -> crate::Result<()> {
        for offset in 0..frame_count {
            self.precompile_frame(composition, start_frame.saturating_add(offset), media)?;
        }
        Ok(())
    }

    pub fn render_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<RasterHandle> {
        self.render_frame_submitted(composition, frame, media)
            .map(|(raster, _submission)| raster)
    }

    pub fn render_frame_submitted<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        self.ensure_compiled_for_frame(composition, frame, Some(media))?;
        let bound = self.bind_frame(composition, frame, media)?;
        self.submit_bound_frame(&bound)
    }

    pub fn bind_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<BoundFrame> {
        self.ensure_compiled_for_frame(composition, frame, Some(media))?;
        let compiled = self.active_compiled()?;
        FrameBindContext::with_media(composition, frame, media).bind(compiled)
    }

    pub fn submit_bound_frame(
        &mut self,
        bound: &BoundFrame,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        self.upload_bound_frame(bound)?;
        self.submit_render()
    }

    pub fn upload_bound_frame(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        self.upload_media_textures(bound)?;
        let update = bound.frame_update();
        let prepared = self.active_prepared_mut()?;
        prepared
            .renderer
            .apply_frame_update(&prepared.compiled.plan, &update)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        Ok(())
    }

    fn upload_media_textures(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        let key = self.active_key.clone().ok_or_else(|| RenderError::Gpu {
            details: "composition has not been compiled".to_string(),
        })?;
        let prepared = self
            .compiled_plans
            .get_mut(&key)
            .ok_or_else(|| RenderError::Gpu {
                details: "composition has not been compiled".to_string(),
            })?;
        let media_texture_cache = &mut self.media_texture_cache;
        for upload in bound.media_textures() {
            if upload.key.frame.is_some() {
                let rgba = fit_frame_to_rgba8(&upload.frame, upload.size.width, upload.size.height);
                prepared.renderer.queue.write_texture(
                    lumen_gpu::wgpu::TexelCopyTextureInfo {
                        texture: prepared.renderer.texture(upload.texture).ok_or_else(|| {
                            RenderError::Gpu {
                                details: format!("unknown media texture {:?}", upload.texture),
                            }
                        })?,
                        mip_level: 0,
                        origin: lumen_gpu::wgpu::Origin3d::ZERO,
                        aspect: lumen_gpu::wgpu::TextureAspect::All,
                    },
                    &rgba,
                    lumen_gpu::wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(upload.size.width * 4),
                        rows_per_image: Some(upload.size.height),
                    },
                    upload.size.as_extent(),
                );
                prepared.current_media_textures.remove(&upload.texture);
                continue;
            }

            if prepared
                .current_media_textures
                .get(&upload.texture)
                .is_some_and(|current| current == &upload.key)
            {
                continue;
            }

            let texture = if let Some(texture) = media_texture_cache.get(&upload.key) {
                Arc::clone(texture)
            } else {
                let texture = Arc::new(prepared.renderer.device.create_texture(
                    &lumen_gpu::wgpu::TextureDescriptor {
                        label: Some("lumen media cached frame"),
                        size: upload.size.as_extent(),
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: lumen_gpu::wgpu::TextureDimension::D2,
                        format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        usage: lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
                            | lumen_gpu::wgpu::TextureUsages::COPY_DST
                            | lumen_gpu::wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    },
                ));
                let rgba = fit_frame_to_rgba8(&upload.frame, upload.size.width, upload.size.height);
                prepared.renderer.queue.write_texture(
                    texture.as_image_copy(),
                    &rgba,
                    lumen_gpu::wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(upload.size.width * 4),
                        rows_per_image: Some(upload.size.height),
                    },
                    upload.size.as_extent(),
                );
                media_texture_cache.insert(upload.key.clone(), Arc::clone(&texture));
                texture
            };

            let desc = lumen_gpu::TextureDesc::sampled(
                upload.size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            );
            prepared
                .renderer
                .replace_texture_arc(upload.texture, texture, desc)
                .map_err(|error| RenderError::Gpu {
                    details: error.to_string(),
                })?;
            prepared
                .current_media_textures
                .insert(upload.texture, upload.key.clone());
        }
        Ok(())
    }

    pub fn submit_render(
        &mut self,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        let prepared = self.active_prepared_mut()?;
        let submission = prepared
            .renderer
            .submit_plan(&prepared.compiled.plan)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        Ok((prepared.compiled.output, submission))
    }

    pub fn gpu_renderer(&self) -> &lumen_gpu::Renderer {
        self.active_key
            .as_ref()
            .and_then(|key| self.compiled_plans.get(key))
            .map(|prepared| &prepared.renderer)
            .unwrap_or(&self.renderer)
    }

    pub fn gpu_renderer_mut(&mut self) -> &mut lumen_gpu::Renderer {
        let Some(key) = self.active_key.clone() else {
            return &mut self.renderer;
        };
        self.compiled_plans
            .get_mut(&key)
            .map(|prepared| &mut prepared.renderer)
            .unwrap_or(&mut self.renderer)
    }

    pub fn compiled(&self) -> Option<&CompiledComposition> {
        self.active_key
            .as_ref()
            .and_then(|key| self.compiled_plans.get(key))
            .map(|prepared| &prepared.compiled)
    }

    fn active_compiled(&self) -> crate::Result<&CompiledComposition> {
        self.compiled().ok_or_else(|| {
            RenderError::Gpu {
                details: "composition has not been compiled".to_string(),
            }
            .into()
        })
    }

    fn active_prepared_mut(&mut self) -> crate::Result<&mut PreparedComposition> {
        let key = self.active_key.clone().ok_or_else(|| RenderError::Gpu {
            details: "composition has not been compiled".to_string(),
        })?;
        self.compiled_plans.get_mut(&key).ok_or_else(|| {
            RenderError::Gpu {
                details: "composition has not been compiled".to_string(),
            }
            .into()
        })
    }

    fn ensure_compiled_for_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: Option<&M>,
    ) -> crate::Result<()> {
        let key = self.compiled_plan_key(composition, frame)?;
        if self.active_key.as_ref() == Some(&key) {
            return Ok(());
        }
        if self.compiled_plans.contains_key(&key) {
            self.active_key = Some(key);
            return Ok(());
        }

        let compiled = match media {
            Some(media) => {
                CompileContext::with_media_for_frame(composition, frame, media, self.output_format)
                    .compile()?
            }
            None => CompileContext::with_frame(composition, frame, self.output_format).compile()?,
        };
        self.prepare_compiled(key, compiled)
    }

    fn compiled_plan_key(
        &self,
        composition: &Composition,
        frame: u32,
    ) -> crate::Result<CompiledPlanKey> {
        let mut selections = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let output = self.media_output_port(composition)?;
        self.collect_plan_key(composition, &output, frame, &mut visited, &mut selections)?;
        selections.sort_by_key(|(node_id, _)| node_id.0);
        selections.dedup();
        Ok(CompiledPlanKey(selections))
    }

    fn collect_plan_key(
        &self,
        composition: &Composition,
        port: &PortRef,
        frame: u32,
        visited: &mut std::collections::HashSet<(NodeId, u32)>,
        selections: &mut Vec<(NodeId, Option<usize>)>,
    ) -> crate::Result<()> {
        if port.is_empty() || !visited.insert((port.id, frame)) {
            return Ok(());
        }

        let Some(node) = composition.graph.nodes.get(&port.id) else {
            return Ok(());
        };
        match node {
            NodeKind::MediaOutput(media_output) => self.collect_plan_key(
                composition,
                &media_output.source,
                frame,
                visited,
                selections,
            ),
            NodeKind::TimeRemap(time_remap) => {
                let ctx = self.expression_context(composition, frame, time_remap.id, "frame");
                let target_frame = crate::node::processing::time_remap::remap_frame(
                    crate::node::processing::time_remap::resolve_settings(
                        time_remap.id,
                        &time_remap.frame,
                        &time_remap.loop_enabled,
                        &time_remap.loop_start,
                        &time_remap.loop_end,
                        &ctx,
                    )?,
                );
                self.collect_plan_key(
                    composition,
                    &time_remap.source,
                    target_frame,
                    visited,
                    selections,
                )
            }
            NodeKind::Switch(switch) => {
                let ctx = self.expression_context(composition, frame, switch.id, "selected_layer");
                let selection =
                    crate::node::compositing::switch::selected_layer_for_frame(switch, &ctx)?;
                selections.push((switch.id, selection));
                if let Some(layer) = selection.and_then(|index| switch.layers.get(index)) {
                    self.collect_plan_key(composition, layer, frame, visited, selections)?;
                }
                Ok(())
            }
            _ => {
                for input in composition
                    .graph
                    .connections
                    .iter()
                    .filter(|connection| connection.to_node == port.id)
                    .map(|connection| {
                        PortRef::new(connection.from_node, connection.from_port.clone())
                    })
                {
                    self.collect_plan_key(composition, &input, frame, visited, selections)?;
                }
                Ok(())
            }
        }
    }

    fn media_output_port(&self, composition: &Composition) -> crate::Result<PortRef> {
        let mut outputs = composition
            .graph
            .nodes
            .iter()
            .filter_map(|(node_id, node)| {
                matches!(node, NodeKind::MediaOutput(_)).then_some(*node_id)
            });
        let Some(output) = outputs.next() else {
            return Err(crate::error::GraphValidationError::MissingMediaOutput.into());
        };
        if outputs.next().is_some() {
            return Err(
                crate::error::GraphValidationError::MultipleMediaOutputs { count: 2 }.into(),
            );
        }
        Ok(PortRef::new(output, "output".to_string()))
    }

    fn expression_context<'a>(
        &self,
        composition: &'a Composition,
        frame: u32,
        node_id: NodeId,
        property_path: &str,
    ) -> crate::expr::ExpressionContext<'a> {
        crate::expr::ExpressionContext {
            frame,
            fps: composition.timeline.fps,
            width: composition.render_settings.width,
            height: composition.render_settings.height,
            duration_frames: composition.timeline.duration_frames,
            path: Some(format!("{node_id}.{property_path}")),
            graph: Some(&composition.graph),
        }
    }
}

fn fit_frame_to_rgba8(frame: &CpuMediaFrame, width: u32, height: u32) -> Vec<u8> {
    if frame.width == width && frame.height == height && frame.row_bytes == width as usize * 4 {
        return frame.rgba.as_ref().clone();
    }

    let mut out = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        let src_y = ((u64::from(y) * u64::from(frame.height)) / u64::from(height)) as usize;
        for x in 0..width {
            let src_x = ((u64::from(x) * u64::from(frame.width)) / u64::from(width)) as usize;
            let src = src_y
                .saturating_mul(frame.row_bytes)
                .saturating_add(src_x.saturating_mul(4));
            let dst = (y as usize)
                .saturating_mul(width as usize * 4)
                .saturating_add(x as usize * 4);
            out[dst..dst + 4].copy_from_slice(&frame.rgba[src..src + 4]);
        }
    }
    out
}
