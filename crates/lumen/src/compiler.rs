use std::collections::HashMap;

use thiserror::Error;

use crate::{
    plan::{
        AssetRenderOp, CanvasSpec, RenderOp, RenderOpKind, RenderPlan, ShapeRenderOp,
        SolidRenderOp, TextRenderOp, VideoRenderOp,
    },
    sequence::{Asset, AssetKind, ClipContent, Sequence, Track, TrackKind},
    time::{FrameIndex, Rational, TimeError, frame_at_time, frames_from_time},
};

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("invalid timeline: {0}")]
    InvalidTimeline(String),
    #[error("invalid canvas: {0}")]
    InvalidCanvas(String),
    #[error("track `{track_id}` has invalid clip `{clip_id}`: {reason}")]
    InvalidClip {
        track_id: String,
        clip_id: String,
        reason: String,
    },
    #[error("missing asset `{asset_id}`")]
    MissingAsset { asset_id: String },
    #[error("asset `{asset_id}` does not match track kind")]
    AssetKindMismatch { asset_id: String },
    #[error("duplicate asset id `{asset_id}`")]
    DuplicateAsset { asset_id: String },
    #[error("audio graph is not implemented in phase 1")]
    UnimplementedAudio,
    #[error("time conversion failed: {0}")]
    Time(#[from] TimeError),
}

pub fn compile_sequence(sequence: &Sequence) -> Result<RenderPlan, CompileError> {
    validate_canvas(sequence.canvas.width, sequence.canvas.height)?;
    validate_timeline(sequence.timeline.fps)?;

    if !sequence.audio.tracks.is_empty() {
        return Err(CompileError::UnimplementedAudio);
    }

    let assets = index_assets(&sequence.assets)?;
    let total_frames = frames_from_time(sequence.timeline.duration, sequence.timeline.fps)?;

    if total_frames == 0 {
        return Err(CompileError::InvalidTimeline(
            "timeline duration resolves to 0 frames".to_string(),
        ));
    }

    let mut operations = Vec::new();

    for (track_index, track) in sequence.tracks.iter().enumerate() {
        if track.kind == TrackKind::Audio && !track.clips.is_empty() {
            return Err(CompileError::UnimplementedAudio);
        }

        compile_track(
            track,
            track_index,
            total_frames,
            &assets,
            sequence.timeline.fps,
            &mut operations,
        )?;
    }

    operations.sort_by_key(|op| (op.start_frame, op.z_index, op.clip_index));

    Ok(RenderPlan::with_operations_index(
        CanvasSpec {
            width: sequence.canvas.width,
            height: sequence.canvas.height,
            background: sequence.canvas.background,
        },
        sequence.timeline.fps,
        sequence.timeline.duration,
        total_frames,
        operations,
    ))
}

fn compile_track(
    track: &Track,
    track_index: usize,
    total_frames: u64,
    assets: &HashMap<&str, &Asset>,
    fps: Rational,
    operations: &mut Vec<RenderOp>,
) -> Result<(), CompileError> {
    for (clip_index, clip) in track.clips.iter().enumerate() {
        let start = frame_at_time(clip.start, fps)?;
        let duration = frames_from_time(clip.duration, fps)?;
        let end =
            FrameIndex(
                start
                    .0
                    .checked_add(duration)
                    .ok_or_else(|| CompileError::InvalidClip {
                        track_id: track.id.clone(),
                        clip_id: clip.id.clone(),
                        reason: "frame range overflowed".to_string(),
                    })?,
            );
        let source_in_frame =
            frame_at_time(clip.source_in.unwrap_or(crate::time::Time::ZERO), fps)?;

        if duration == 0 {
            return Err(CompileError::InvalidClip {
                track_id: track.id.clone(),
                clip_id: clip.id.clone(),
                reason: "duration resolves to 0 frames".to_string(),
            });
        }

        if !clip.speed.is_finite() || clip.speed <= 0.0 {
            return Err(CompileError::InvalidClip {
                track_id: track.id.clone(),
                clip_id: clip.id.clone(),
                reason: "speed must be a finite number greater than 0".to_string(),
            });
        }

        if !clip.transform.x.is_finite() || !clip.transform.y.is_finite() {
            return Err(CompileError::InvalidClip {
                track_id: track.id.clone(),
                clip_id: clip.id.clone(),
                reason: "transform coordinates must be finite numbers".to_string(),
            });
        }

        if let Some(width) = clip.transform.width {
            if !width.is_finite() || width <= 0.0 {
                return Err(CompileError::InvalidClip {
                    track_id: track.id.clone(),
                    clip_id: clip.id.clone(),
                    reason: "transform width must be a finite number greater than 0".to_string(),
                });
            }
        }

        if let Some(height) = clip.transform.height {
            if !height.is_finite() || height <= 0.0 {
                return Err(CompileError::InvalidClip {
                    track_id: track.id.clone(),
                    clip_id: clip.id.clone(),
                    reason: "transform height must be a finite number greater than 0".to_string(),
                });
            }
        }

        if end.0 > total_frames {
            return Err(CompileError::InvalidClip {
                track_id: track.id.clone(),
                clip_id: clip.id.clone(),
                reason: format!(
                    "end frame {} exceeds timeline length {}",
                    end.0, total_frames
                ),
            });
        }

        let kind = match (&track.kind, &clip.content) {
            (TrackKind::Text, ClipContent::Text(text)) => RenderOpKind::Text(TextRenderOp {
                text: text.text.clone(),
                font_family: text.font_family.clone(),
                font_size: text.font_size,
                color: text.color,
                align: text.align,
            }),
            (TrackKind::Shape, ClipContent::Shape(shape)) => RenderOpKind::Shape(ShapeRenderOp {
                shape: shape.clone(),
            }),
            (TrackKind::Text, ClipContent::Solid { color }) => {
                RenderOpKind::Solid(SolidRenderOp { color: *color })
            }
            (TrackKind::Image, ClipContent::AssetRef { asset_id }) => {
                RenderOpKind::Image(AssetRenderOp {
                    asset_id: validate_asset_kind(asset_id, AssetKind::Image, assets)?,
                })
            }
            (TrackKind::Video, ClipContent::AssetRef { asset_id }) => {
                RenderOpKind::Video(VideoRenderOp {
                    asset_id: validate_asset_kind(asset_id, AssetKind::Video, assets)?,
                    speed: clip.speed,
                    reverse: clip.reverse,
                    source_span_frames: source_span(duration, clip.speed),
                })
            }
            (TrackKind::Image, ClipContent::Solid { color })
            | (TrackKind::Video, ClipContent::Solid { color }) => {
                RenderOpKind::Solid(SolidRenderOp { color: *color })
            }
            (TrackKind::Audio, _) => return Err(CompileError::UnimplementedAudio),
            _ => {
                return Err(CompileError::InvalidClip {
                    track_id: track.id.clone(),
                    clip_id: clip.id.clone(),
                    reason: "content type does not match track kind".to_string(),
                });
            }
        };

        if track.kind != TrackKind::Video
            && (clip.reverse || (clip.speed - 1.0).abs() > f32::EPSILON)
        {
            return Err(CompileError::InvalidClip {
                track_id: track.id.clone(),
                clip_id: clip.id.clone(),
                reason: "speed/reverse are only supported for video clips".to_string(),
            });
        }

        operations.push(RenderOp {
            id: clip.id.clone(),
            start_frame: start,
            end_frame: end,
            source_in_frame,
            z_index: track_index as u32,
            clip_index,
            opacity: clip.opacity.clamp(0.0, 1.0),
            blend_mode: clip.blend_mode,
            transform: clip.transform,
            kind,
        });
    }

    Ok(())
}

fn validate_asset_kind(
    asset_id: &str,
    expected: AssetKind,
    assets: &HashMap<&str, &Asset>,
) -> Result<String, CompileError> {
    let asset = assets
        .get(asset_id)
        .copied()
        .ok_or_else(|| CompileError::MissingAsset {
            asset_id: asset_id.to_string(),
        })?;

    if asset.kind != expected {
        return Err(CompileError::AssetKindMismatch {
            asset_id: asset_id.to_string(),
        });
    }

    Ok(asset.id.clone())
}

fn index_assets(assets: &[Asset]) -> Result<HashMap<&str, &Asset>, CompileError> {
    let mut indexed = HashMap::new();
    for asset in assets {
        if indexed.insert(asset.id.as_str(), asset).is_some() {
            return Err(CompileError::DuplicateAsset {
                asset_id: asset.id.clone(),
            });
        }
    }

    Ok(indexed)
}

fn validate_canvas(width: u32, height: u32) -> Result<(), CompileError> {
    if width == 0 || height == 0 {
        return Err(CompileError::InvalidCanvas(
            "canvas dimensions must be > 0".to_string(),
        ));
    }

    if width % 2 != 0 || height % 2 != 0 {
        return Err(CompileError::InvalidCanvas(
            "canvas dimensions must be even for yuv420p output".to_string(),
        ));
    }

    Ok(())
}

fn validate_timeline(fps: Rational) -> Result<(), CompileError> {
    if fps.num == 0 || fps.den == 0 {
        return Err(CompileError::InvalidTimeline(
            "fps must be greater than 0".to_string(),
        ));
    }

    Ok(())
}

fn source_span(duration_frames: u64, speed: f32) -> u64 {
    ((duration_frames as f64) * speed as f64).ceil().max(1.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sequence::{
            AudioGraph, BlendMode, Canvas, ClipContent, Sequence, TextAlign, TextContent, Timeline,
            Track, TrackClip, TrackKind, Transform,
        },
        time::Time,
    };

    #[test]
    fn deterministic_plan_ordering() {
        let sequence = sample_sequence();
        let first = compile_sequence(&sequence).unwrap();
        let second = compile_sequence(&sequence).unwrap();

        assert_eq!(first.operations.len(), second.operations.len());
        for (left, right) in first.operations.iter().zip(second.operations.iter()) {
            assert_eq!(left.id, right.id);
            assert_eq!(left.start_frame, right.start_frame);
            assert_eq!(left.z_index, right.z_index);
        }
    }

    #[test]
    fn rejects_clip_outside_timeline() {
        let mut sequence = sample_sequence();
        sequence.tracks[0].clips[0].duration = Time::new(90, 30).unwrap();

        let result = compile_sequence(&sequence);

        assert!(matches!(result, Err(CompileError::InvalidClip { .. })));
    }

    #[test]
    fn rejects_non_finite_transform() {
        let mut sequence = sample_sequence();
        sequence.tracks[0].clips[0].transform.x = f32::NAN;

        let result = compile_sequence(&sequence);

        assert!(matches!(result, Err(CompileError::InvalidClip { .. })));
    }

    fn sample_sequence() -> Sequence {
        Sequence {
            canvas: Canvas {
                width: 320,
                height: 180,
                background: crate::sequence::ColorRGBA(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).unwrap(),
                duration: Time::new(60, 30).unwrap(),
            },
            assets: vec![],
            tracks: vec![Track {
                id: "text-track".to_string(),
                kind: TrackKind::Text,
                clips: vec![TrackClip {
                    id: "clip-a".to_string(),
                    start: Time::new(0, 30).unwrap(),
                    duration: Time::new(30, 30).unwrap(),
                    source_in: None,
                    speed: 1.0,
                    reverse: false,
                    transform: Transform::default(),
                    opacity: 1.0,
                    blend_mode: BlendMode::Normal,
                    content: ClipContent::Text(TextContent {
                        text: "hello".to_string(),
                        font_family: None,
                        font_size: 20.0,
                        color: crate::sequence::ColorRGBA(255, 255, 255, 255),
                        align: TextAlign::Center,
                    }),
                }],
            }],
            audio: AudioGraph::default(),
        }
    }
}
