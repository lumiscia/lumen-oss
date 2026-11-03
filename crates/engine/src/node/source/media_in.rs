use std::ops::Range;

use crate::error::{MediaError, RenderError};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};
use crate::scope::FrameTime;

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
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "media_in", name = "Media In", category = "source")]
pub struct MediaIn {
    pub id: NodeId,
    #[params]
    pub params: MediaInParamsDelegate,
}

pub fn resolve_for_context(
    media_in: &MediaIn,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<MediaInKind> {
    let params = media_in.params.eval(&DelegateEvalContext {
        node_id: media_in.id,
        expr: ctx,
    })?;
    let kind = params.kind;
    let source = params.source;
    let range_start = params.range_start;
    let range_end = params.range_end;
    let speed = params.speed as f32;
    let loop_mode = LoopMode::from_int(params.loop_mode);

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

/// Maps a (possibly fractional) composition frame to the source frame shown at that instant.
/// The source position is floored, so a frame stays on screen until the next one starts.
///
/// Frame rates are `f32`, so a rate with no exact binary form, like NTSC's 30000/1001, converts
/// whole frames only to within `f64` rounding.
pub fn map_to_source_frame(
    frame: impl Into<FrameTime>,
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
    let frame = frame.into();
    let comp_fps = composition_fps.max(1.0);
    let media_fps = if source_fps.is_finite() && source_fps > 0.0 {
        source_fps
    } else {
        comp_fps
    };
    // The time is divided once, after it is multiplied out, so whole frames convert exactly.
    // Matching rates skip the conversion, which can round a frame just below an integer (e.g.
    // a looped `time_remap` target) up onto the next one.
    let scale = speed.abs() as f64;
    let relative = if media_fps == comp_fps {
        (frame.num() * scale) / frame.den()
    } else {
        (frame.num() * media_fps as f64 * scale) / (frame.den() * comp_fps as f64)
    }
    .floor() as u32;
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

        let size = ctx
            .media()
            .and_then(|media| self.native_size(&ctx.expr_context(), media))
            .unwrap_or_else(|| ctx.canvas_size());
        let texture = ctx.builder_mut().texture(
            Some(format!("media-in:{}:frame", self.id.0)),
            lumen_gpu::TextureDesc::sampled(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
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
            bounds: None,
        }))
    }
}

impl MediaIn {
    /// The size of the media the node shows at `ctx`'s frame, which its texture is compiled to.
    /// `None` when the media can't be opened, which binding the node reports.
    pub(crate) fn native_size(
        &self,
        ctx: &crate::expr::ExpressionContext<'_>,
        media: &dyn crate::media::MediaStore,
    ) -> Option<lumen_gpu::Size> {
        match resolve_for_context(self, ctx).ok()? {
            MediaInKind::Image { image_id } => {
                let metadata = media.get_image_resolver(&image_id).ok()?.metadata();
                Some(lumen_gpu::Size::new(
                    metadata.width.max(1),
                    metadata.height.max(1),
                ))
            }
            MediaInKind::Video { stream_id, .. } => {
                let metadata = media.get_video_resolver(&stream_id).ok()?.metadata();
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
            node_id: self.node_id,
            node_kind: "MediaIn",
            details: "media store is required for media input nodes".to_string(),
        })?;
        let node = MediaIn {
            id: self.node_id,
            params: self.params.clone(),
        };
        let kind = resolve_for_context(&node, &ctx.expr_context())?;
        let (frame, key) = match kind {
            MediaInKind::Image { image_id } => {
                let resolver = media.get_image_resolver(&image_id)?;
                let key = MediaTextureKey {
                    source: resolver.id().to_string(),
                    width: self.size.width,
                    height: self.size.height,
                };
                (resolver.frame()?, Some(key))
            }
            MediaInKind::Video {
                stream_id,
                range,
                speed,
                loop_mode,
            } => {
                let resolver = media.get_video_resolver(&stream_id)?;
                let metadata = resolver.metadata();
                let source_frame = map_to_source_frame(
                    ctx.frame_time(),
                    ctx.expr_context().fps,
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
                (resolver.frame(source_frame)?, None)
            }
        };
        bound.use_media_texture(self.texture, key, frame, self.size);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{LoopMode, map_to_source_frame};

    struct Row {
        name: &'static str,
        composition_fps: f32,
        source_fps: f32,
        speed: f32,
        mode: LoopMode,
        range: Option<std::ops::Range<u32>>,
        frame_count: u32,
        frames: Vec<(f64, u32)>,
    }

    fn forward(name: &'static str, source_fps: f32, frames: Vec<(f64, u32)>) -> Row {
        Row {
            name,
            composition_fps: 30.0,
            source_fps,
            speed: 1.0,
            mode: LoopMode::Clamp,
            range: None,
            frame_count: 100,
            frames,
        }
    }

    fn ping_pong(name: &'static str, speed: f32, sources: [u32; 8]) -> Row {
        Row {
            speed,
            mode: LoopMode::PingPong,
            frame_count: 4,
            frames: (0..).map(f64::from).zip(sources).collect(),
            ..forward(name, 30.0, Vec::new())
        }
    }

    #[test]
    fn composition_frames_map_to_source_frames() {
        let rows = [
            forward(
                "lower fps media repeats frames",
                15.0,
                vec![(0.0, 0), (1.0, 0), (2.0, 1), (3.0, 1), (4.0, 2), (5.0, 2)],
            ),
            forward(
                "higher fps media skips frames",
                60.0,
                vec![(0.0, 0), (1.0, 2), (2.0, 4), (2.9, 5)],
            ),
            forward(
                "a fractional frame shows the source frame at that instant",
                30.0,
                vec![(1.0, 1), (1.5, 1), (1.999, 1)],
            ),
            forward(
                "a frame just below a source frame stays on the one before",
                15.0,
                vec![(6.0_f64.next_down(), 2)],
            ),
            Row {
                speed: -1.0,
                range: Some(2..7),
                frame_count: 10,
                ..forward(
                    "negative speed plays the range in reverse and clamps",
                    30.0,
                    vec![(0.0, 6), (3.0, 3), (99.0, 2)],
                )
            },
            ping_pong(
                "ping pong bounces off both ends",
                1.0,
                [0, 1, 2, 3, 2, 1, 0, 1],
            ),
            ping_pong(
                "negative ping pong starts at the end",
                -1.0,
                [3, 2, 1, 0, 1, 2, 3, 2],
            ),
        ];
        for row in rows {
            for (frame, expected) in row.frames {
                let mapped = map_to_source_frame(
                    frame,
                    row.composition_fps,
                    row.source_fps,
                    row.frame_count,
                    row.range.as_ref(),
                    row.speed,
                    row.mode,
                );
                assert_eq!(mapped, Some(expected), "{}: frame {frame}", row.name);
            }
        }
    }

    #[test]
    fn looped_remap_target_keeps_its_source_frame() {
        use crate::node::processing::time_remap::{TimeRemapSettings, remap_frame};
        let target = remap_frame(
            crate::node::NodeId::new(1),
            TimeRemapSettings {
                frame: 1.0 / 49.0 * 49.0,
                loop_enabled: true,
                loop_start: 1,
                loop_end: 2,
            },
        )
        .unwrap();
        for fps in [24.0, 30.0, 60.0] {
            assert_eq!(
                map_to_source_frame(target, fps, fps, 10, None, 1.0, LoopMode::Clamp),
                Some(1)
            );
        }
    }
}
