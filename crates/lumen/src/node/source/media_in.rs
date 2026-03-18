use std::ops::Range;

use crate::{
    error::{LumenError, MediaError},
    media::MediaStore,
    node::{NodeId, NodeProperty},
    raster::RasterFrame,
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Repeat,
    PingPong,
}

impl LoopMode {
    fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Repeat,
            2 => Self::PingPong,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaInKind {
    Image {
        source: String,
    },
    Video {
        source: String,
        range: Option<Range<u32>>,
        speed: f32,
        loop_mode: LoopMode,
    },
}

#[derive(Debug, Clone, Node)]
pub struct MediaIn {
    pub id: NodeId,

    #[property(expected = Int)]
    pub kind: NodeProperty,
    #[property(expected = String)]
    pub source: NodeProperty,
    #[property(expected = Int)]
    pub range_start: NodeProperty,
    #[property(expected = Int)]
    pub range_end: NodeProperty,
    #[property(expected = Float)]
    pub speed: NodeProperty,
    #[property(expected = Int)]
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

#[node_impl]
impl MediaIn {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let kind = self.resolve_kind(ctx)?;
        let source = self.resolve_source(ctx)?;
        let range_start = self.resolve_range_start(ctx)?;
        let range_end = self.resolve_range_end(ctx)?;
        let speed = self.resolve_speed(ctx)? as f32;
        let loop_mode = LoopMode::from_int(self.resolve_loop_mode(ctx)?);

        let media = if kind == 0 {
            MediaInKind::Image { source }
        } else {
            MediaInKind::Video {
                source,
                range: resolve_range(range_start, range_end),
                speed,
                loop_mode,
            }
        };

        match &media {
            MediaInKind::Image { source } => evaluate_image(source, ctx),
            MediaInKind::Video {
                source,
                range,
                speed,
                loop_mode,
            } => evaluate_video(source, range.as_ref(), *speed, *loop_mode, ctx),
        }
    }
}

fn resolve_range(start: i64, end: i64) -> Option<Range<u32>> {
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(end).ok()?;
    (end > start).then_some(start..end)
}

fn evaluate_image<S: SurfacePool, M: MediaStore>(
    source: &str,
    ctx: &mut RenderContext<'_, S, M>,
) -> crate::Result<RasterFrame> {
    let resolver = ctx
        .renderer
        .media_store
        .get_image_resolver(source)
        .ok_or_else(|| MediaError::SourceNotFound {
            media_source: source.to_string(),
        })?;
    let meta = resolver.metadata();
    let width = meta.width.max(1);
    let height = meta.height.max(1);

    // TODO: asset_cache not available on new RenderContext
    let decoded = resolver.resolve()?;

    validate_rgba_len(source, width, height, decoded.as_ref())?;
    Ok(RasterFrame::bitmap(decoded, width, height))
}

fn evaluate_video<S: SurfacePool, M: MediaStore>(
    source: &str,
    range: Option<&Range<u32>>,
    speed: f32,
    loop_mode: LoopMode,
    ctx: &mut RenderContext<'_, S, M>,
) -> crate::Result<RasterFrame> {
    let resolver = ctx
        .renderer
        .media_store
        .get_video_resolver(source)
        .ok_or_else(|| MediaError::SourceNotFound {
            media_source: source.to_string(),
        })?;
    let meta = resolver.metadata();
    let width = meta.width.max(1);
    let height = meta.height.max(1);
    let frame_count = meta.frame_count;

    // TODO: asset_cache not available on new RenderContext

    let source_frame = map_to_source_frame(ctx.frame, frame_count, range, speed, loop_mode).ok_or(
        MediaError::FrameOutOfRange {
            media_source: source.to_string(),
            frame: ctx.frame,
            frame_count,
        },
    )?;

    let decoded = resolver.resolve_frame(source_frame)?;
    validate_rgba_len(source, width, height, decoded.as_ref())?;
    Ok(RasterFrame::bitmap(decoded, width, height))
}

fn map_to_source_frame(
    timeline_frame: u32,
    frame_count: u32,
    range: Option<&Range<u32>>,
    speed: f32,
    loop_mode: LoopMode,
) -> Option<u32> {
    if frame_count == 0 {
        return None;
    }

    let (start, end) = range
        .map(|trim| (trim.start.min(frame_count), trim.end.min(frame_count)))
        .unwrap_or((0, frame_count));
    if end <= start {
        return None;
    }

    let duration = u64::from(end - start);
    let playback_speed = if speed.is_finite() && speed.abs() > f32::EPSILON {
        speed as f64
    } else {
        1.0
    };
    let reverse = playback_speed.is_sign_negative();
    let stepped = (f64::from(timeline_frame) * playback_speed.abs())
        .floor()
        .max(0.0) as u64;

    let forward_offset = match loop_mode {
        LoopMode::None => stepped.min(duration.saturating_sub(1)),
        LoopMode::Repeat => stepped % duration,
        LoopMode::PingPong => {
            let cycle = duration.saturating_mul(2);
            let pos = if cycle == 0 { 0 } else { stepped % cycle };
            if pos < duration {
                pos
            } else {
                cycle.saturating_sub(pos).saturating_sub(1)
            }
        }
    };

    let offset = if reverse {
        duration.saturating_sub(1).saturating_sub(forward_offset)
    } else {
        forward_offset
    };
    u32::try_from(u64::from(start).saturating_add(offset)).ok()
}

fn validate_rgba_len(
    source: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), MediaError> {
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|count| count.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| MediaError::Decode {
            media_source: source.to_string(),
            details: "invalid frame dimensions".to_string(),
        })?;

    if pixels.len() == expected {
        Ok(())
    } else {
        Err(MediaError::Decode {
            media_source: source.to_string(),
            details: format!(
                "expected rgba buffer length {expected}, got {}",
                pixels.len()
            ),
        })
    }
}
