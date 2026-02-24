use std::ops::Range;

use crate::{
    error::{LumenError, MediaError},
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    raster::RasterFrame,
    render::RenderContext,
};

const INPUT_PORTS: [InputPortDef; 0] = [];

const OUTPUT_PORTS: [OutputPortDef; 1] = [OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Repeat,
    PingPong,
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

#[derive(Debug, Clone, PartialEq)]
pub struct MediaIn {
    pub kind: MediaInKind,
}

impl NodeEval for MediaIn {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &INPUT_PORTS
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &OUTPUT_PORTS
    }

    fn evaluate(
        &self,
        _inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let frame = match &self.kind {
            MediaInKind::Image { source } => evaluate_image(source, ctx)?,
            MediaInKind::Video {
                source,
                range,
                speed,
                loop_mode,
            } => evaluate_video(source, range.as_ref(), *speed, *loop_mode, ctx)?,
        };
        Ok(PortValue::RasterFrame(frame))
    }
}

fn evaluate_image(source: &str, ctx: &mut RenderContext) -> Result<RasterFrame, LumenError> {
    let resolver =
        ctx.media_store
            .get_image_resolver(source)
            .ok_or_else(|| MediaError::SourceNotFound {
                media_source: source.to_string(),
            })?;
    let width = resolver.width().max(1);
    let height = resolver.height().max(1);

    let decoded = {
        let mut cache = ctx.asset_cache.write().map_err(|_| MediaError::Decode {
            media_source: source.to_string(),
            details: "asset cache lock poisoned".to_string(),
        })?;
        cache.get_or_insert_image(source, resolver.as_ref())?
    };

    validate_rgba_len(source, width, height, decoded.as_ref())?;
    Ok(RasterFrame::bitmap(decoded, width, height))
}

fn evaluate_video(
    source: &str,
    range: Option<&Range<u32>>,
    speed: f32,
    loop_mode: LoopMode,
    ctx: &mut RenderContext,
) -> Result<RasterFrame, LumenError> {
    let resolver =
        ctx.media_store
            .get_video_resolver(source)
            .ok_or_else(|| MediaError::SourceNotFound {
                media_source: source.to_string(),
            })?;
    let width = resolver.width().max(1);
    let height = resolver.height().max(1);
    let frame_count = resolver.frame_count();

    {
        let mut cache = ctx.asset_cache.write().map_err(|_| MediaError::Decode {
            media_source: source.to_string(),
            details: "asset cache lock poisoned".to_string(),
        })?;
        cache.set_video_metadata(
            source.to_string(),
            crate::cache::VideoMetadata {
                width,
                height,
                frame_count,
            },
        );
    }

    let source_frame = map_to_source_frame(ctx.request.frame, frame_count, range, speed, loop_mode)
        .ok_or(MediaError::FrameOutOfRange {
            media_source: source.to_string(),
            frame: ctx.request.frame,
            frame_count,
        })?;

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
