use std::ops::Range;

use crate::error::{MediaError, RenderError};
use crate::media::{CpuMediaFrame, MediaFrame};
use crate::node::{NodeId, NodeProperty};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, RasterMetadata,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    Clamp,
    Repeat,
}

impl LoopMode {
    pub fn from_int(value: i64) -> Self {
        if value == 1 {
            Self::Repeat
        } else {
            Self::Clamp
        }
    }
}

#[derive(Debug, Clone)]
pub enum MediaInKind {
    Image {
        image_id: String,
    },
    Video {
        stream_id: String,
        range: Option<Range<u32>>,
        speed: f32,
        loop_mode: LoopMode,
    },
}

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "media_in",
    label = "Media In",
    description = "Binds an external image or video frame as a GPU texture.",
    category = "source"
)]
pub struct MediaIn {
    pub id: NodeId,
    #[property(kind = "int")]
    pub kind: NodeProperty,
    #[property(kind = "string")]
    pub source: NodeProperty,
    #[property(kind = "int")]
    pub range_start: NodeProperty,
    #[property(kind = "int")]
    pub range_end: NodeProperty,
    #[property(kind = "float")]
    pub speed: NodeProperty,
    #[property(kind = "int")]
    pub loop_mode: NodeProperty,
}

impl Default for MediaIn {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            kind: NodeProperty::Int(0),
            source: NodeProperty::String(String::new()),
            range_start: NodeProperty::Int(0),
            range_end: NodeProperty::Int(0),
            speed: NodeProperty::Float(1.0),
            loop_mode: NodeProperty::Int(0),
        }
    }
}

pub fn resolve_for_context(
    media_in: &MediaIn,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<MediaInKind> {
    let kind = media_in.kind.resolve_int(media_in.id, "kind", ctx)?;
    let source = media_in.source.resolve_string(media_in.id, "source", ctx)?;
    let range_start = media_in
        .range_start
        .resolve_int(media_in.id, "range_start", ctx)?;
    let range_end = media_in
        .range_end
        .resolve_int(media_in.id, "range_end", ctx)?;
    let speed = media_in.speed.resolve_float(media_in.id, "speed", ctx)? as f32;
    let loop_mode = LoopMode::from_int(media_in.loop_mode.resolve_int(
        media_in.id,
        "loop_mode",
        ctx,
    )?);

    if kind == 1 {
        Ok(MediaInKind::Video {
            stream_id: source,
            range: resolve_range(range_start, range_end),
            speed,
            loop_mode,
        })
    } else {
        Ok(MediaInKind::Image { image_id: source })
    }
}

pub fn resolve_range(start: i64, end: i64) -> Option<Range<u32>> {
    if end <= start {
        return None;
    }
    Some(start.max(0) as u32..end.max(0) as u32)
}

pub fn map_to_source_frame(
    frame: u32,
    composition_fps: f32,
    source_fps: f32,
    frame_count: u32,
    range: Option<&Range<u32>>,
    speed: f32,
    loop_mode: LoopMode,
) -> Option<u32> {
    if frame_count == 0 {
        return None;
    }
    let start = range.map(|range| range.start).unwrap_or(0);
    let end = range
        .map(|range| range.end)
        .unwrap_or(frame_count)
        .min(frame_count);
    if end <= start {
        return None;
    }
    let span = end - start;
    let comp_fps = composition_fps.max(1.0);
    let media_fps = source_fps.max(comp_fps);
    let relative = (((frame as f64 / comp_fps as f64) * media_fps as f64) * speed.max(0.0) as f64)
        .floor() as u32;
    let mapped = match loop_mode {
        LoopMode::Clamp => start + relative.min(span.saturating_sub(1)),
        LoopMode::Repeat => start + (relative % span),
    };
    (mapped < frame_count).then_some(mapped)
}

impl GpuCompileNode for MediaIn {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &crate::node::PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let size = self.native_size(ctx).unwrap_or_else(|| {
            lumen_gpu::Size::new(
                ctx.composition().render_settings.width.max(1),
                ctx.composition().render_settings.height.max(1),
            )
        });
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("media-in:{}:frame", self.id.0)),
            lumen_gpu::TextureDesc::sampled(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Texture(texture),
        );
        ctx.push_frame_binding(FrameBinding::MediaInput {
            node_id: self.id,
            source: self.source.clone(),
            texture,
            size,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: RasterMetadata::default(),
        }))
    }
}

impl MediaIn {
    fn native_size(&self, ctx: &crate::gpu::CompileContext<'_>) -> Option<lumen_gpu::Size> {
        let media = ctx.media()?;
        let kind = resolve_for_context(self, &ctx.expr_context(self.id, "source")).ok()?;
        match kind {
            MediaInKind::Image { image_id } => {
                let metadata = media.get_image_resolver(&image_id)?.metadata();
                Some(lumen_gpu::Size::new(
                    metadata.width.max(1),
                    metadata.height.max(1),
                ))
            }
            MediaInKind::Video { stream_id, .. } => {
                let metadata = media.get_video_resolver(&stream_id)?.metadata();
                Some(lumen_gpu::Size::new(
                    metadata.width.max(1),
                    metadata.height.max(1),
                ))
            }
        }
    }
}

impl GpuFrameBindNode for MediaIn {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::MediaInput {
            node_id,
            texture,
            size,
            ..
        } = binding
        else {
            return Ok(());
        };
        let media = ctx.media().ok_or_else(|| RenderError::NodeEvaluation {
            frame: ctx.frame(),
            node_id: *node_id,
            node_kind: "MediaIn",
            details: "media store is required for media input nodes".to_string(),
        })?;
        let kind = resolve_for_context(self, &ctx.expr_context(*node_id, "source"))?;
        let frame = match kind {
            MediaInKind::Image { image_id } => media
                .get_image_resolver(&image_id)
                .ok_or_else(|| MediaError::SourceNotFound {
                    media_source: image_id.clone(),
                })?
                .frame()?,
            MediaInKind::Video {
                stream_id,
                range,
                speed,
                loop_mode,
            } => {
                let resolver = media.get_video_resolver(&stream_id).ok_or_else(|| {
                    MediaError::SourceNotFound {
                        media_source: stream_id.clone(),
                    }
                })?;
                let metadata = resolver.metadata();
                let source_frame = map_to_source_frame(
                    ctx.frame(),
                    ctx.expr_context(*node_id, "source").fps,
                    metadata.fps,
                    metadata.frame_count,
                    range.as_ref(),
                    speed,
                    loop_mode,
                )
                .ok_or_else(|| MediaError::FrameOutOfRange {
                    media_source: stream_id.clone(),
                    frame: ctx.frame(),
                    frame_count: metadata.frame_count,
                })?;
                resolver.frame(source_frame)?
            }
        };
        let MediaFrame::CpuRgba(frame) = frame else {
            return Err(RenderError::NodeEvaluation {
                frame: ctx.frame(),
                node_id: *node_id,
                node_kind: "MediaIn",
                details:
                    "external texture media frames are not imported by the local GPU renderer yet"
                        .to_string(),
            }
            .into());
        };
        let rgba = fit_frame_to_rgba8(&frame, size.width, size.height);
        bound.write_texture_rgba8(*texture, rgba, size.width * 4, size.height);
        Ok(())
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
