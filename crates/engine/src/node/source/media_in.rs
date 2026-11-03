use std::ops::Range;

use crate::error::{MediaError, RenderError};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};
use crate::scope::FrameTime;

use crate::gpu::{
    BoundFrame, FrameBindContext, GpuCompileNode, GpuCompiledNode, MediaTextureKey, RasterHandle,
};

/// What playback does once it leaves the source range.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum LoopMode {
    #[default]
    Clamp = 0,
    Repeat = 1,
    PingPong = 2,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum MediaInSourceKind {
    #[default]
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
        range: Range<u32>,
        speed: f32,
        loop_mode: LoopMode,
    },
}

/// Binds an external image or video frame as a GPU texture.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct MediaInParams {
    /// Type of external media source.
    #[meta(kind = "enum", name = "Media type", enum_type = MediaInSourceKind)]
    pub kind: MediaInSourceKind,
    /// External media source identifier.
    #[meta(role = "source_id")]
    pub source: String,
    /// First source frame to include.
    #[meta(min = 0, step = 1)]
    pub range_start: i64,
    /// First source frame past the range; the source's end when unset.
    #[meta(min = 0, step = 1)]
    pub range_end: Option<i64>,
    /// Playback speed multiplier.
    #[meta(step = 0.1)]
    pub speed: f64,
    /// Behavior when playback leaves the source range.
    #[meta(kind = "enum", enum_type = LoopMode)]
    pub loop_mode: LoopMode,
}

impl Default for MediaInParams {
    fn default() -> Self {
        Self {
            kind: MediaInSourceKind::Image,
            source: String::new(),
            range_start: 0,
            range_end: None,
            speed: 1.0,
            loop_mode: LoopMode::Clamp,
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

impl MediaIn {
    /// What the node shows at `ctx`'s frame.
    pub fn resolve(&self, ctx: &crate::expr::ExpressionContext<'_>) -> crate::Result<MediaInKind> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: ctx,
        })?;
        Ok(match params.kind {
            MediaInSourceKind::Image => MediaInKind::Image {
                image_id: params.source,
            },
            MediaInSourceKind::Video => MediaInKind::Video {
                stream_id: params.source,
                range: saturate_frame(params.range_start)
                    ..params.range_end.map_or(u32::MAX, saturate_frame),
                speed: params.speed as f32,
                loop_mode: params.loop_mode,
            },
        })
    }
}

fn saturate_frame(frame: i64) -> u32 {
    u32::try_from(frame).unwrap_or(if frame < 0 { 0 } else { u32::MAX })
}

/// Maps a (possibly fractional) composition frame to the source frame shown at that instant.
/// The source position is floored, so a frame stays on screen until the next one starts.
///
/// `range` is the source frames to play, up to but not including its end, which is clamped to
/// the source's `frame_count`.
///
/// Frame rates are `f32`, so a rate with no exact binary form, like NTSC's 30000/1001, converts
/// whole frames only to within `f64` rounding.
pub fn map_to_source_frame(
    frame: impl Into<FrameTime>,
    composition_fps: f32,
    source_fps: f32,
    frame_count: u32,
    range: &Range<u32>,
    speed: f32,
    loop_mode: LoopMode,
) -> Option<u32> {
    let start = i64::from(range.start);
    let end = i64::from(range.end.min(frame_count));
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
    .floor() as i64;
    let offset = match loop_mode {
        LoopMode::Clamp => relative.clamp(0, span - 1),
        LoopMode::Repeat => relative.rem_euclid(span),
        LoopMode::PingPong => {
            let period = (span * 2 - 2).max(1);
            let offset = relative.rem_euclid(period);
            if offset < span {
                offset
            } else {
                period - offset
            }
        }
    };
    let offset = if speed >= 0.0 {
        offset
    } else {
        span - 1 - offset
    };
    u32::try_from(start + offset).ok()
}

impl GpuCompileNode for MediaIn {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let size = self
            .native_size(&ctx.expr_context())?
            .unwrap_or_else(|| ctx.canvas_size());
        let texture = ctx.builder_mut().texture(
            Some(format!("media-in:{}:frame", self.id.0)),
            lumen_gpu::TextureDesc::sampled(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        ctx.register_compiled_node(CompiledMediaInput {
            media_in: self.clone(),
            texture,
            size,
        });

        Ok(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}

impl MediaIn {
    /// The size of the media the node shows at `ctx`'s frame, which its texture is compiled to.
    /// `None` without a media store.
    pub(crate) fn native_size(
        &self,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Option<lumen_gpu::Size>> {
        let Some(media) = ctx.media else {
            return Ok(None);
        };
        let (width, height) = match self.resolve(ctx)? {
            MediaInKind::Image { image_id } => {
                let metadata = media.get_image_resolver(&image_id)?.metadata();
                (metadata.width, metadata.height)
            }
            MediaInKind::Video { stream_id, .. } => {
                let metadata = media.get_video_resolver(&stream_id)?.metadata();
                (metadata.width, metadata.height)
            }
        };
        Ok(Some(lumen_gpu::Size::new(width.max(1), height.max(1))))
    }
}

#[derive(Debug, Clone)]
struct CompiledMediaInput {
    media_in: MediaIn,
    texture: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
}

impl GpuCompiledNode for CompiledMediaInput {
    fn node_id(&self) -> NodeId {
        self.media_in.id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let media = ctx.media().ok_or_else(|| RenderError::NodeEvaluation {
            node_id: self.media_in.id,
            node_kind: "MediaIn",
            details: "media store is required for media input nodes".to_string(),
        })?;
        let (frame, key) = match self.media_in.resolve(&ctx.expr_context())? {
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
                    &range,
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
        range: std::ops::Range<u32>,
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
            range: 0..u32::MAX,
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
                range: 2..7,
                frame_count: 10,
                ..forward(
                    "negative speed plays the range in reverse and clamps",
                    30.0,
                    vec![(0.0, 6), (3.0, 3), (99.0, 2)],
                )
            },
            Row {
                range: 4..u32::MAX,
                frame_count: 10,
                ..forward(
                    "a range with only a start plays from it to the source's end",
                    30.0,
                    vec![(0.0, 4), (5.0, 9), (50.0, 9)],
                )
            },
            Row {
                range: 2..5,
                mode: LoopMode::Repeat,
                frame_count: 10,
                ..forward(
                    "the range's end is the first frame past it",
                    30.0,
                    vec![(0.0, 2), (2.0, 4), (3.0, 2), (-1.0, 4)],
                )
            },
            Row {
                range: 2..3,
                mode: LoopMode::PingPong,
                frame_count: 10,
                ..forward(
                    "a one-frame range ping-pongs in place",
                    30.0,
                    vec![(0.0, 2), (1.0, 2), (7.0, 2)],
                )
            },
            Row {
                range: 5..50,
                mode: LoopMode::Repeat,
                frame_count: 8,
                ..forward(
                    "a range past the source's end is cut to it",
                    30.0,
                    vec![(0.0, 5), (2.0, 7), (3.0, 5)],
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
                    &row.range,
                    row.speed,
                    row.mode,
                );
                assert_eq!(mapped, Some(expected), "{}: frame {frame}", row.name);
            }
        }
    }

    #[test]
    fn an_empty_range_maps_no_frame() {
        for range in [3..3, 20..30] {
            assert_eq!(
                map_to_source_frame(0.0, 30.0, 30.0, 10, &range, 1.0, LoopMode::Clamp),
                None,
                "{range:?}"
            );
        }
    }

    #[test]
    fn looped_remap_target_keeps_its_source_frame() {
        use crate::node::processing::time_remap::{TimeRemapParams, remap_frame};
        let params = TimeRemapParams {
            frame: 1.0 / 49.0 * 49.0,
            loop_enabled: true,
            loop_start: 1,
            loop_end: 2,
        };
        let target = remap_frame(crate::node::NodeId::new(1), &params).unwrap();
        for fps in [24.0, 30.0, 60.0] {
            assert_eq!(
                map_to_source_frame(target, fps, fps, 10, &(0..u32::MAX), 1.0, LoopMode::Clamp),
                Some(1)
            );
        }
    }
}
