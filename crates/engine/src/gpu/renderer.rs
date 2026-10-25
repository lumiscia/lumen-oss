use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    composition::Composition,
    error::RenderError,
    gpu::{
        BoundFrame, CompileContext, CompiledComposition, FrameBindContext, MediaTextureKey,
        RasterHandle,
    },
    media::{CpuMediaFrame, MediaFrame, MediaStore},
    node::{NodeId, NodeKind, NodeParamEvalContext, NodeParams, PortRef},
};

#[cfg(feature = "ffmpeg")]
use super::media::GpuMediaFrameImporter;

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
    bind_sequence: u64,
    text_atlas: Arc<Mutex<lumen_text::GlyphAtlas>>,
    text_atlas_texture: Option<Arc<lumen_gpu::wgpu::Texture>>,
    output_format: lumen_gpu::wgpu::TextureFormat,
    media_texture_cache: HashMap<MediaTextureKey, Arc<lumen_gpu::wgpu::Texture>>,
    #[cfg(feature = "ffmpeg")]
    gpu_media_importer: GpuMediaFrameImporter,
}

impl GpuCompositionRenderer {
    pub async fn new() -> crate::Result<Self> {
        let renderer = lumen_gpu::Renderer::new()
            .await
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        tracing::debug!(target: "lumen_render", "created gpu composition renderer");
        Ok(Self {
            renderer,
            compiled_plans: HashMap::new(),
            active_key: None,
            bind_sequence: 0,
            text_atlas: Arc::new(Mutex::new(lumen_text::GlyphAtlas::new(
                super::types::TEXT_ATLAS_SIZE,
            ))),
            text_atlas_texture: None,
            output_format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            media_texture_cache: HashMap::new(),
            #[cfg(feature = "ffmpeg")]
            gpu_media_importer: GpuMediaFrameImporter::default(),
        })
    }

    pub fn from_device(device: lumen_gpu::wgpu::Device, queue: lumen_gpu::wgpu::Queue) -> Self {
        Self {
            renderer: lumen_gpu::Renderer::from_device(device, queue),
            compiled_plans: HashMap::new(),
            active_key: None,
            bind_sequence: 0,
            text_atlas: Arc::new(Mutex::new(lumen_text::GlyphAtlas::new(
                super::types::TEXT_ATLAS_SIZE,
            ))),
            text_atlas_texture: None,
            output_format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            media_texture_cache: HashMap::new(),
            #[cfg(feature = "ffmpeg")]
            gpu_media_importer: GpuMediaFrameImporter::default(),
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
        let key = self.compiled_plan_key(composition, 0, None)?;
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

    /// Drops every compiled plan and cached media texture, destroying the GPU textures and
    /// buffers this renderer exclusively owns instead of waiting for them to be garbage collected.
    /// Retains the shared text atlas (64 MiB once allocated) and cached glyphs across recompilation.
    pub fn release_resources(&mut self) {
        self.bind_sequence += 1;
        for texture in self.media_texture_cache.values() {
            texture.destroy();
        }
        for prepared in self.compiled_plans.values() {
            for resource in prepared.compiled.plan.textures() {
                if let Some(texture) = prepared.renderer.texture_arc(resource.id)
                    && Arc::strong_count(&texture) == 2
                {
                    texture.destroy();
                }
            }
            for resource in prepared.compiled.plan.buffers() {
                if let Some(buffer) = prepared.renderer.buffer(resource.id) {
                    buffer.destroy();
                }
            }
        }
        self.media_texture_cache.clear();
        self.compiled_plans.clear();
        self.active_key = None;
    }

    /// Drops compiled plans, cached media, and the text atlas, reclaiming GPU memory and cached glyphs.
    pub fn release_text_atlas(&mut self) {
        self.release_resources();
        if let Some(texture) = self.text_atlas_texture.take() {
            texture.destroy();
        }
        self.text_atlas = Arc::new(Mutex::new(lumen_text::GlyphAtlas::new(
            super::types::TEXT_ATLAS_SIZE,
        )));
    }

    fn reset_compiled(&mut self, output_format: lumen_gpu::wgpu::TextureFormat) {
        tracing::debug!(
            target: "lumen_render",
            ?output_format,
            cached_plans = self.compiled_plans.len(),
            media_textures = self.media_texture_cache.len(),
            "reset compiled composition"
        );
        self.bind_sequence += 1;
        self.output_format = output_format;
        self.compiled_plans.clear();
        self.active_key = None;
    }

    fn prepare_compiled(
        &mut self,
        key: CompiledPlanKey,
        mut compiled: CompiledComposition,
    ) -> crate::Result<()> {
        tracing::debug!(
            target: "lumen_render",
            key_entries = key.0.len(),
            textures = compiled.plan.textures().len(),
            buffers = compiled.plan.buffers().len(),
            programs = compiled.plan.programs().len(),
            passes = compiled.plan.passes().len(),
            compiled_nodes = compiled.compiled_nodes.len(),
            "prepare compiled render plan"
        );
        let mut renderer = lumen_gpu::Renderer::from_device_with_adapter_info(
            self.renderer.device.clone(),
            self.renderer.queue.clone(),
            self.renderer.adapter_info().clone(),
        );
        renderer
            .prepare_plan(&compiled.plan)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        compiled.text_atlas = Arc::clone(&self.text_atlas);
        if let Some(id) = compiled.text_atlas_texture {
            self.ensure_text_atlas_texture()?;
            renderer
                .replace_texture_arc(
                    id,
                    Arc::clone(self.text_atlas_texture.as_ref().expect("atlas texture")),
                    super::types::text_atlas_desc(),
                )
                .map_err(|error| RenderError::Gpu {
                    details: error.to_string(),
                })?
                .destroy();
        }
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
        tracing::trace!(
            target: "lumen_render",
            start_frame,
            frame_count,
            "precompile frame window"
        );
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
        let span = tracing::trace_span!(target: "lumen_render", "render frame", frame);
        let _entered = span.enter();
        let bound = self.bind_frame(composition, frame, media)?;
        tracing::trace!(
            target: "lumen_render",
            frame,
            buffer_uploads = bound.buffer_upload_count(),
            texture_uploads = bound.texture_upload_count(),
            media_textures = bound.media_textures().len(),
            "bound frame"
        );
        self.submit_bound_frame(&bound)
    }

    pub fn render_frame_into_external<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
        external: lumen_gpu::ExternalTexture,
    ) -> crate::Result<lumen_gpu::SubmittedExternalTexture> {
        let span = tracing::trace_span!(target: "lumen_render", "render frame into external texture", frame);
        let _entered = span.enter();
        let bound = self.bind_frame(composition, frame, media)?;
        self.upload_bound_frame(&bound)?;
        let prepared = self.active_prepared_mut()?;
        prepared
            .renderer
            .submit_plan_with_external_texture(
                &prepared.compiled.plan,
                prepared.compiled.output.texture,
                external,
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })
            .map_err(Into::into)
    }

    pub fn bind_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<BoundFrame> {
        self.bind_sequence += 1;
        self.ensure_compiled_for_frame(composition, frame, Some(media))?;
        let compiled = self.active_compiled()?;
        let mut bound = FrameBindContext::with_media(composition, frame, media).bind(compiled)?;
        bound.renderer_binding = self.active_key.clone().map(|key| (self.bind_sequence, key));
        Ok(bound)
    }

    pub fn submit_bound_frame(
        &mut self,
        bound: &BoundFrame,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        self.upload_bound_frame(bound)?;
        self.submit_render()
    }

    pub fn upload_bound_frame(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        tracing::trace!(
            target: "lumen_render",
            buffer_uploads = bound.buffer_upload_count(),
            texture_uploads = bound.texture_upload_count(),
            media_textures = bound.media_textures().len(),
            "upload bound frame"
        );
        if bound
            .renderer_binding
            .as_ref()
            .is_none_or(|(sequence, key)| {
                *sequence != self.bind_sequence || Some(key) != self.active_key.as_ref()
            })
        {
            return Err(RenderError::Gpu {
                details: "stale bound text frame; rebind before uploading".to_string(),
            }
            .into());
        }
        if bound.text_atlas.is_some() {
            self.ensure_text_atlas_texture()?;
        }
        self.upload_media_textures(bound)?;
        let update = bound.frame_update();
        let prepared = self.active_prepared_mut()?;
        prepared
            .renderer
            .apply_frame_update(&prepared.compiled.plan, &update)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        bound.mark_uploaded()?;
        Ok(())
    }

    fn ensure_text_atlas_texture(&mut self) -> crate::Result<()> {
        if self.text_atlas_texture.is_some() {
            return Ok(());
        }
        let desc = super::types::text_atlas_desc();
        let texture = Arc::new(super::types::create_text_atlas_texture(
            &self.renderer.device,
        ));
        for prepared in self.compiled_plans.values_mut() {
            if let Some(id) = prepared.compiled.text_atlas_texture {
                prepared
                    .renderer
                    .replace_texture_arc(id, Arc::clone(&texture), desc)
                    .map_err(|error| RenderError::Gpu {
                        details: error.to_string(),
                    })?;
            }
        }
        self.text_atlas_texture = Some(texture);
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
        #[cfg(feature = "ffmpeg")]
        let gpu_media_importer = &mut self.gpu_media_importer;
        for upload in bound.media_textures() {
            tracing::trace!(
                target: "lumen_render",
                texture = ?upload.texture,
                source = %upload.key.source,
                frame = ?upload.key.frame,
                width = upload.size.width,
                height = upload.size.height,
                "resolve media texture upload"
            );
            #[cfg(feature = "ffmpeg")]
            if let MediaFrame::GpuVideo(frame) = &upload.frame {
                let texture = gpu_media_importer
                    .import(&prepared.renderer, upload, frame)
                    .map_err(|details| RenderError::Gpu {
                        details: format!("failed to import GPU media frame: {details}"),
                    })?;
                let desc = lumen_gpu::TextureDesc::sampled(
                    upload.size,
                    match frame.frame.backend() {
                        #[cfg(all(target_os = "macos", feature = "metal"))]
                        lumen_ffmpeg::GpuBackend::Metal => {
                            lumen_gpu::wgpu::TextureFormat::Bgra8Unorm
                        }
                        _ => lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                    },
                );
                prepared
                    .renderer
                    .replace_texture_arc(upload.texture, texture, desc)
                    .map_err(|error| RenderError::Gpu {
                        details: error.to_string(),
                    })?;
                prepared.current_media_textures.remove(&upload.texture);
                continue;
            }

            let MediaFrame::CpuRgba(frame) = &upload.frame else {
                return Err(RenderError::Gpu {
                    details: "media frame backend is not supported by this GPU renderer build"
                        .to_string(),
                }
                .into());
            };

            if upload.key.frame.is_some() {
                tracing::trace!(
                    target: "lumen_render",
                    texture = ?upload.texture,
                    source = %upload.key.source,
                    frame = ?upload.key.frame,
                    "upload uncached video frame"
                );
                let rgba = fit_frame_to_rgba8(frame, upload.size.width, upload.size.height);
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
                tracing::trace!(
                    target: "lumen_render",
                    texture = ?upload.texture,
                    source = %upload.key.source,
                    frame = ?upload.key.frame,
                    "reuse current media texture binding"
                );
                continue;
            }

            let texture = if let Some(texture) = media_texture_cache.get(&upload.key) {
                tracing::trace!(
                    target: "lumen_render",
                    source = %upload.key.source,
                    frame = ?upload.key.frame,
                    "reuse cached media texture"
                );
                Arc::clone(texture)
            } else {
                tracing::debug!(
                    target: "lumen_render",
                    source = %upload.key.source,
                    frame = ?upload.key.frame,
                    width = upload.size.width,
                    height = upload.size.height,
                    "cache media texture"
                );
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
                let rgba = fit_frame_to_rgba8(frame, upload.size.width, upload.size.height);
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
            let displaced = prepared
                .renderer
                .replace_texture_arc(upload.texture, texture, desc)
                .map_err(|error| RenderError::Gpu {
                    details: error.to_string(),
                })?;
            // The plan's own placeholder is not shared, so free it now rather than at GC on web.
            if Arc::strong_count(&displaced) == 1 {
                displaced.destroy();
            }
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
        tracing::trace!(
            target: "lumen_render",
            passes = prepared.compiled.plan.passes().len(),
            "submit render"
        );
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
        let key = self.compiled_plan_key(
            composition,
            frame,
            media.map(|store| store as &dyn MediaStore),
        )?;
        if self.active_key.as_ref() == Some(&key) {
            tracing::trace!(target: "lumen_render", frame, "reuse active compiled plan");
            return Ok(());
        }
        if self.compiled_plans.contains_key(&key) {
            tracing::debug!(
                target: "lumen_render",
                frame,
                key_entries = key.0.len(),
                "switch to cached compiled plan"
            );
            self.active_key = Some(key);
            return Ok(());
        }

        tracing::debug!(
            target: "lumen_render",
            frame,
            key_entries = key.0.len(),
            "compile render plan for frame"
        );
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
        media: Option<&dyn MediaStore>,
    ) -> crate::Result<CompiledPlanKey> {
        let mut selections = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let output = self.media_output_port(composition)?;
        self.collect_plan_key(
            composition,
            &output,
            frame,
            media,
            &mut visited,
            &mut selections,
        )?;
        selections.sort_by_key(|(node_id, _)| node_id.0);
        selections.dedup();
        Ok(CompiledPlanKey(selections))
    }

    fn collect_plan_key(
        &self,
        composition: &Composition,
        port: &PortRef,
        frame: u32,
        media: Option<&dyn MediaStore>,
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
                media,
                visited,
                selections,
            ),
            NodeKind::TimeRemap(time_remap) => {
                let ctx =
                    self.expression_context(composition, frame, media, time_remap.id, "params");
                let params = time_remap.params.eval(&NodeParamEvalContext {
                    node_id: time_remap.id,
                    expr: &ctx,
                })?;
                let target_frame = crate::node::processing::time_remap::remap_frame(
                    crate::node::processing::time_remap::TimeRemapSettings {
                        frame: params.frame,
                        loop_enabled: params.loop_enabled,
                        loop_start: params.loop_start,
                        loop_end: params.loop_end,
                    },
                );
                self.collect_plan_key(
                    composition,
                    &time_remap.source,
                    target_frame,
                    media,
                    visited,
                    selections,
                )
            }
            NodeKind::Switch(switch) => {
                let ctx =
                    self.expression_context(composition, frame, media, switch.id, "selected_layer");
                let selection =
                    crate::node::compositing::switch::selected_layer_for_frame(switch, &ctx)?;
                selections.push((switch.id, selection));
                if let Some(layer) = selection.and_then(|index| switch.layers.get(index)) {
                    self.collect_plan_key(composition, layer, frame, media, visited, selections)?;
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
                    self.collect_plan_key(composition, &input, frame, media, visited, selections)?;
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
        media: Option<&'a dyn MediaStore>,
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
            media,
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
