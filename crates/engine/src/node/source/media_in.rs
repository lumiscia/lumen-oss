use std::ops::Range;

use crate::error::{MediaError, RenderError};
use crate::node::{NodeId, NodeParams};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, MediaTextureKey,
    RasterHandle, RasterMetadata,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, lumen_macros::NodeEnum)]
pub enum LoopMode {
    Clamp,
    Repeat,
    PingPong,
}

impl LoopMode {
    pub fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Repeat,
            2 => Self::PingPong,
            _ => Self::Clamp,
        }
    }
}

#[cfg(any(feature = "json", feature = "metadata"))]
#[derive(lumen_macros::NodeEnum)]
pub enum MediaInSourceKind {
    Image = 0,
    Video = 1,
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

/// Binds an external image or video frame as a GPU texture.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct MediaInParams {
    /// Type of external media source.
    #[meta(kind = "enum", name = "Media type", enum_type = MediaInSourceKind)]
    pub kind: i64,
    /// External media source identifier.
    #[meta(role = "source_id")]
    pub source: String,
    /// First source frame to include.
    #[meta(min = 0, step = 1)]
    pub range_start: i64,
    /// Last source frame to include.
    #[meta(min = 0, step = 1)]
    pub range_end: i64,
    /// Playback speed multiplier.
    #[meta(step = 0.1)]
    pub speed: f64,
    /// Behavior when playback leaves the source range.
    #[meta(kind = "enum", enum_type = LoopMode)]
    pub loop_mode: i64,
}

impl Default for MediaInParams {
    fn default() -> Self {
        Self {
            kind: 0,
            source: String::new(),
            range_start: 0,
            range_end: 0,
            speed: 1.0,
            loop_mode: 0,
        }
    }
}

/// Binds an external image or video frame as a GPU texture.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "media_in", name = "Media In", category = "source")]
pub struct MediaIn {
    pub id: NodeId,
    #[params]
    pub params: MediaInParamsDelegate,
}

impl Default for MediaIn {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: MediaInParamsDelegate::default(),
        }
    }
}

pub fn resolve_for_context(
    media_in: &MediaIn,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<MediaInKind> {
    let kind = media_in.params.kind.resolve_int(media_in.id, "kind", ctx)?;
    let source = media_in
        .params
        .source
        .resolve_string(media_in.id, "source", ctx)?;
    let range_start = media_in
        .params
        .range_start
        .resolve_int(media_in.id, "range_start", ctx)?;
    let range_end = media_in
        .params
        .range_end
        .resolve_int(media_in.id, "range_end", ctx)?;
    let speed = media_in
        .params
        .speed
        .resolve_float(media_in.id, "speed", ctx)? as f32;
    let loop_mode = LoopMode::from_int(media_in.params.loop_mode.resolve_int(
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
    let relative =
        (((frame as f64 / comp_fps as f64) * media_fps as f64) * speed.abs() as f64).floor() as u32;
    let forward = speed >= 0.0;
    let mapped = match loop_mode {
        LoopMode::Clamp => {
            if forward {
                start + relative.min(span.saturating_sub(1))
            } else {
                end - 1 - relative.min(span.saturating_sub(1))
            }
        }
        LoopMode::Repeat => {
            let offset = relative % span;
            start + if forward { offset } else { span - 1 - offset }
        }
        LoopMode::PingPong => {
            let period = span.saturating_mul(2).saturating_sub(2).max(1);
            let offset = relative % period;
            let offset = if offset < span {
                offset
            } else {
                period - offset
            };
            start + if forward { offset } else { span - 1 - offset }
        }
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
        ctx.register_compiled_node(CompiledMediaInput {
            node_id: self.id,
            params: self.params.clone(),
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

#[derive(Debug, Clone)]
struct CompiledMediaInput {
    node_id: NodeId,
    params: MediaInParamsDelegate,
    texture: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
}

impl GpuCompiledNode for CompiledMediaInput {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let media = ctx.media().ok_or_else(|| RenderError::NodeEvaluation {
            frame: ctx.frame(),
            node_id: self.node_id,
            node_kind: "MediaIn",
            details: "media store is required for media input nodes".to_string(),
        })?;
        let node = MediaIn {
            id: self.node_id,
            params: self.params.clone(),
        };
        let kind = resolve_for_context(&node, &ctx.expr_context(self.node_id, "source"))?;
        let (frame, key_source, key_frame) = match kind {
            MediaInKind::Image { image_id } => media
                .get_image_resolver(&image_id)
                .ok_or_else(|| MediaError::SourceNotFound {
                    media_source: image_id.clone(),
                })?
                .frame()
                .map(|frame| (frame, image_id, None))?,
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
                    ctx.expr_context(self.node_id, "source").fps,
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
                resolver
                    .frame(source_frame)
                    .map(|frame| (frame, stream_id, Some(source_frame)))?
            }
        };
        bound.use_media_texture(
            self.texture,
            MediaTextureKey {
                source: key_source,
                frame: key_frame,
                width: self.size.width,
                height: self.size.height,
            },
            frame,
            self.size,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{LoopMode, map_to_source_frame};

    #[test]
    fn maps_negative_media_speed_in_reverse() {
        assert_eq!(
            map_to_source_frame(0, 30.0, 30.0, 10, Some(&(2..7)), -1.0, LoopMode::Clamp),
            Some(6)
        );
        assert_eq!(
            map_to_source_frame(3, 30.0, 30.0, 10, Some(&(2..7)), -1.0, LoopMode::Clamp),
            Some(3)
        );
        assert_eq!(
            map_to_source_frame(99, 30.0, 30.0, 10, Some(&(2..7)), -1.0, LoopMode::Clamp),
            Some(2)
        );
    }

    #[test]
    fn maps_ping_pong_loop_mode() {
        let frames = (0..8)
            .map(|frame| map_to_source_frame(frame, 30.0, 30.0, 4, None, 1.0, LoopMode::PingPong))
            .collect::<Vec<_>>();
        assert_eq!(
            frames,
            vec![
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(2),
                Some(1),
                Some(0),
                Some(1)
            ]
        );
    }

    #[test]
    fn maps_negative_ping_pong_loop_mode() {
        let frames = (0..8)
            .map(|frame| map_to_source_frame(frame, 30.0, 30.0, 4, None, -1.0, LoopMode::PingPong))
            .collect::<Vec<_>>();
        assert_eq!(
            frames,
            vec![
                Some(3),
                Some(2),
                Some(1),
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(2)
            ]
        );
    }
}
