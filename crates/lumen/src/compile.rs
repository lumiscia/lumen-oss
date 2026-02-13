use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{
    model::{
        Canvas, ClipContent, Easing, FitMode, Layer, Project, ScalarKeyframe, ShapeClip, Source,
        SourceMediaType, SourcePipeline, TextClip, Timeline, Transform,
    },
    source_pipeline::{PipelineError, map_source_frame},
};

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("invalid canvas: {0}")]
    InvalidCanvas(String),
    #[error("invalid timeline: {0}")]
    InvalidTimeline(String),
    #[error("duplicate source id `{0}`")]
    DuplicateSourceId(String),
    #[error("missing source `{0}`")]
    MissingSource(String),
    #[error(
        "source `{source_id}` has incompatible media type: expected {expected:?}, found {found:?}"
    )]
    SourceTypeMismatch {
        source_id: String,
        expected: SourceMediaType,
        found: SourceMediaType,
    },
    #[error("invalid clip `{clip_id}` in layer `{layer_id}`: {reason}")]
    InvalidClip {
        layer_id: String,
        clip_id: String,
        reason: String,
    },
    #[error("source pipeline error: {0}")]
    Pipeline(#[from] PipelineError),
}

#[derive(Debug, Clone)]
pub struct CompiledTimeline {
    pub canvas: Canvas,
    pub timeline: Timeline,
    sources: HashMap<String, Source>,
    operations: Vec<CompiledOperation>,
    frame_index: Vec<Vec<usize>>,
}

impl CompiledTimeline {
    pub fn total_frames(&self) -> u64 {
        self.timeline.total_frames
    }

    pub fn operation_indices_for_frame(&self, frame: u64) -> Result<&[usize], CompileError> {
        let frame_index = self.frame_index.get(frame as usize).ok_or_else(|| {
            CompileError::InvalidTimeline(format!("frame {frame} is out of range"))
        })?;
        Ok(frame_index.as_slice())
    }

    pub fn operation(&self, index: usize) -> Option<&CompiledOperation> {
        self.operations.get(index)
    }

    pub fn source(&self, source_id: &str) -> Option<&Source> {
        self.sources.get(source_id)
    }

    pub fn sources(&self) -> impl Iterator<Item = &Source> {
        self.sources.values()
    }
}

#[derive(Debug, Clone)]
pub struct CompiledOperation {
    pub id: String,
    pub layer_id: String,
    pub start_frame: u64,
    pub end_frame: u64,
    pub z_index: i32,
    pub opacity: f32,
    pub transform: Transform,
    pub animation: CompiledClipAnimation,
    pub kind: CompiledOperationKind,
}

impl CompiledOperation {
    pub fn contains_frame(&self, frame: u64) -> bool {
        frame >= self.start_frame && frame < self.end_frame
    }

    pub fn local_frame(&self, frame: u64) -> u64 {
        frame.saturating_sub(self.start_frame)
    }

    pub fn resolved_opacity(&self, frame: u64) -> f32 {
        let local = self.local_frame(frame);
        evaluate_scalar_track(self.opacity, &self.animation.opacity, local).clamp(0.0, 1.0)
    }

    pub fn resolved_transform(&self, frame: u64) -> Transform {
        let local = self.local_frame(frame);
        let mut transform = self.transform;
        transform.x = evaluate_scalar_track(transform.x, &self.animation.x, local);
        transform.y = evaluate_scalar_track(transform.y, &self.animation.y, local);
        transform.rotation_degrees = evaluate_scalar_track(
            transform.rotation_degrees,
            &self.animation.rotation_degrees,
            local,
        );
        if let Some(width) = transform.width {
            transform.width = Some(evaluate_scalar_track(width, &self.animation.width, local));
        }
        if let Some(height) = transform.height {
            transform.height = Some(evaluate_scalar_track(height, &self.animation.height, local));
        }
        transform
    }

    pub fn resolve_video_source_frame(&self, frame: u64) -> Result<Option<u64>, CompileError> {
        match &self.kind {
            CompiledOperationKind::Video(video) => {
                let local = self.local_frame(frame);
                map_source_frame(&video.pipeline, local).map_err(CompileError::from)
            }
            _ => Ok(None),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CompiledOperationKind {
    Solid { color: crate::model::ColorRgba },
    Shape(ShapeClip),
    Text(TextClip),
    Image(ImageSourceRef),
    Video(VideoSourceRef),
}

#[derive(Debug, Clone)]
pub struct ImageSourceRef {
    pub source_id: String,
    pub fit: FitMode,
    pub corner_radius: f32,
}

#[derive(Debug, Clone)]
pub struct VideoSourceRef {
    pub source_id: String,
    pub pipeline: SourcePipeline,
    pub fit: FitMode,
    pub corner_radius: f32,
}

#[derive(Debug, Clone, Default)]
pub struct CompiledClipAnimation {
    pub opacity: Vec<CompiledScalarKeyframe>,
    pub x: Vec<CompiledScalarKeyframe>,
    pub y: Vec<CompiledScalarKeyframe>,
    pub width: Vec<CompiledScalarKeyframe>,
    pub height: Vec<CompiledScalarKeyframe>,
    pub rotation_degrees: Vec<CompiledScalarKeyframe>,
}

#[derive(Debug, Clone, Copy)]
pub struct CompiledScalarKeyframe {
    pub frame: u64,
    pub value: f32,
    pub duration_frames: u64,
    pub easing: Easing,
}

fn evaluate_scalar_track(base: f32, track: &[CompiledScalarKeyframe], local_frame: u64) -> f32 {
    let mut current = base;

    for keyframe in track {
        if local_frame < keyframe.frame {
            break;
        }

        let from = current;
        if keyframe.duration_frames == 0 {
            current = keyframe.value;
            continue;
        }

        let end = keyframe.frame.saturating_add(keyframe.duration_frames);
        if local_frame >= end {
            current = keyframe.value;
            continue;
        }

        let progress =
            (local_frame.saturating_sub(keyframe.frame)) as f32 / (keyframe.duration_frames as f32);
        let eased = apply_easing(progress, keyframe.easing);
        return from + (keyframe.value - from) * eased;
    }

    current
}

fn apply_easing(progress: f32, easing: Easing) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        Easing::EaseIn => t * t,
        Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
    }
}

pub fn compile_project(project: &Project) -> Result<CompiledTimeline, CompileError> {
    validate_canvas(&project.canvas)?;
    validate_timeline(&project.timeline)?;

    let sources = index_sources(&project.sources)?;

    let mut staged = Vec::new();
    let mut sequence = 0usize;

    for layer in &project.layers {
        compile_layer(
            layer,
            project.timeline.total_frames,
            &sources,
            &mut staged,
            &mut sequence,
        )?;
    }

    staged.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let operations: Vec<CompiledOperation> = staged.into_iter().map(|(_, _, op)| op).collect();

    let mut frame_index = vec![Vec::new(); project.timeline.total_frames as usize];
    for (index, operation) in operations.iter().enumerate() {
        for frame in operation.start_frame..operation.end_frame.min(project.timeline.total_frames) {
            if let Some(slot) = frame_index.get_mut(frame as usize) {
                slot.push(index);
            }
        }
    }

    Ok(CompiledTimeline {
        canvas: project.canvas.clone(),
        timeline: project.timeline.clone(),
        sources,
        operations,
        frame_index,
    })
}

fn validate_canvas(canvas: &Canvas) -> Result<(), CompileError> {
    if canvas.width == 0 || canvas.height == 0 {
        return Err(CompileError::InvalidCanvas(
            "width and height must be greater than 0".to_string(),
        ));
    }

    Ok(())
}

fn validate_timeline(timeline: &Timeline) -> Result<(), CompileError> {
    if timeline.fps.num == 0 || timeline.fps.den == 0 {
        return Err(CompileError::InvalidTimeline(
            "fps numerator and denominator must be greater than 0".to_string(),
        ));
    }

    if timeline.total_frames == 0 {
        return Err(CompileError::InvalidTimeline(
            "total_frames must be greater than 0".to_string(),
        ));
    }

    Ok(())
}

fn index_sources(sources: &[Source]) -> Result<HashMap<String, Source>, CompileError> {
    let mut seen = HashSet::new();
    let mut map = HashMap::new();

    for source in sources {
        if !seen.insert(source.id.clone()) {
            return Err(CompileError::DuplicateSourceId(source.id.clone()));
        }
        map.insert(source.id.clone(), source.clone());
    }

    Ok(map)
}

fn compile_layer(
    layer: &Layer,
    total_frames: u64,
    sources: &HashMap<String, Source>,
    staged: &mut Vec<(i32, usize, CompiledOperation)>,
    sequence: &mut usize,
) -> Result<(), CompileError> {
    for clip in &layer.clips {
        if clip.duration_frames == 0 {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "duration_frames must be greater than 0".to_string(),
            });
        }

        if !clip.opacity.is_finite() || clip.opacity < 0.0 {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "opacity must be a finite number >= 0".to_string(),
            });
        }

        if !clip.transform.x.is_finite()
            || !clip.transform.y.is_finite()
            || !clip.transform.rotation_degrees.is_finite()
        {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "transform values must be finite".to_string(),
            });
        }

        if let Some(width) = clip.transform.width {
            if !width.is_finite() || width <= 0.0 {
                return Err(CompileError::InvalidClip {
                    layer_id: layer.id.clone(),
                    clip_id: clip.id.clone(),
                    reason: "transform width must be finite and greater than 0".to_string(),
                });
            }
        }

        if let Some(height) = clip.transform.height {
            if !height.is_finite() || height <= 0.0 {
                return Err(CompileError::InvalidClip {
                    layer_id: layer.id.clone(),
                    clip_id: clip.id.clone(),
                    reason: "transform height must be finite and greater than 0".to_string(),
                });
            }
        }

        let end_frame = clip
            .start_frame
            .checked_add(clip.duration_frames)
            .ok_or_else(|| CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "frame range overflow".to_string(),
            })?;

        if end_frame > total_frames {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: format!("clip ends at frame {end_frame} beyond timeline {total_frames}"),
            });
        }

        let animation = compile_clip_animation(layer, clip)?;
        let kind = compile_clip_content(layer, clip, sources)?;

        let operation = CompiledOperation {
            id: clip.id.clone(),
            layer_id: layer.id.clone(),
            start_frame: clip.start_frame,
            end_frame,
            z_index: layer.z_index,
            opacity: clip.opacity.clamp(0.0, 1.0),
            transform: clip.transform,
            animation,
            kind,
        };

        staged.push((layer.z_index, *sequence, operation));
        *sequence = sequence.saturating_add(1);
    }

    Ok(())
}

fn compile_clip_animation(
    layer: &Layer,
    clip: &crate::model::Clip,
) -> Result<CompiledClipAnimation, CompileError> {
    if clip.transform.width.is_none() && !clip.animation.width.is_empty() {
        return Err(CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: "animation.width requires transform.width".to_string(),
        });
    }
    if clip.transform.height.is_none() && !clip.animation.height.is_empty() {
        return Err(CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: "animation.height requires transform.height".to_string(),
        });
    }

    if clip.transform.width.is_some() {
        validate_dimension_keyframes(
            layer,
            clip,
            "animation.width",
            clip.animation.width.as_slice(),
        )?;
    }
    if clip.transform.height.is_some() {
        validate_dimension_keyframes(
            layer,
            clip,
            "animation.height",
            clip.animation.height.as_slice(),
        )?;
    }

    Ok(CompiledClipAnimation {
        opacity: compile_scalar_track(
            layer,
            clip,
            "animation.opacity",
            clip.animation.opacity.as_slice(),
        )?,
        x: compile_scalar_track(layer, clip, "animation.x", clip.animation.x.as_slice())?,
        y: compile_scalar_track(layer, clip, "animation.y", clip.animation.y.as_slice())?,
        width: compile_scalar_track(
            layer,
            clip,
            "animation.width",
            clip.animation.width.as_slice(),
        )?,
        height: compile_scalar_track(
            layer,
            clip,
            "animation.height",
            clip.animation.height.as_slice(),
        )?,
        rotation_degrees: compile_scalar_track(
            layer,
            clip,
            "animation.rotation_degrees",
            clip.animation.rotation_degrees.as_slice(),
        )?,
    })
}

fn validate_dimension_keyframes(
    layer: &Layer,
    clip: &crate::model::Clip,
    field: &str,
    keyframes: &[ScalarKeyframe],
) -> Result<(), CompileError> {
    for keyframe in keyframes {
        if !keyframe.value.is_finite() || keyframe.value <= 0.0 {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: format!("{field} values must be finite and > 0"),
            });
        }
    }
    Ok(())
}

fn compile_scalar_track(
    layer: &Layer,
    clip: &crate::model::Clip,
    field: &str,
    keyframes: &[ScalarKeyframe],
) -> Result<Vec<CompiledScalarKeyframe>, CompileError> {
    let mut sorted = keyframes.to_vec();
    sorted.sort_by_key(|keyframe| keyframe.frame);

    let mut compiled = Vec::with_capacity(sorted.len());
    let mut previous_end = 0u64;

    for (index, keyframe) in sorted.into_iter().enumerate() {
        if !keyframe.value.is_finite() {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: format!("{field}[{index}] value must be finite"),
            });
        }
        if keyframe.frame >= clip.duration_frames {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: format!(
                    "{field}[{index}] frame {} is out of clip range {}",
                    keyframe.frame, clip.duration_frames
                ),
            });
        }

        let end = keyframe
            .frame
            .checked_add(keyframe.duration_frames)
            .ok_or_else(|| CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: format!("{field}[{index}] frame range overflow"),
            })?;
        if end > clip.duration_frames {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: format!(
                    "{field}[{index}] ends at {} beyond clip duration {}",
                    end, clip.duration_frames
                ),
            });
        }
        if index > 0 && keyframe.frame < previous_end {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: format!("{field} keyframes overlap"),
            });
        }

        previous_end = end.max(keyframe.frame);
        compiled.push(CompiledScalarKeyframe {
            frame: keyframe.frame,
            value: keyframe.value,
            duration_frames: keyframe.duration_frames,
            easing: keyframe.easing,
        });
    }

    Ok(compiled)
}

fn compile_clip_content(
    layer: &Layer,
    clip: &crate::model::Clip,
    sources: &HashMap<String, Source>,
) -> Result<CompiledOperationKind, CompileError> {
    match &clip.content {
        ClipContent::Solid { color } => Ok(CompiledOperationKind::Solid { color: *color }),
        ClipContent::Shape(shape) => Ok(CompiledOperationKind::Shape(shape.clone())),
        ClipContent::Text(text) => Ok(CompiledOperationKind::Text(text.clone())),
        ClipContent::Image(image) => {
            validate_source_type(layer, clip, sources, &image.source, SourceMediaType::Image)?;
            if !image.corner_radius.is_finite() || image.corner_radius < 0.0 {
                return Err(CompileError::InvalidClip {
                    layer_id: layer.id.clone(),
                    clip_id: clip.id.clone(),
                    reason: "image corner_radius must be finite and >= 0".to_string(),
                });
            }
            Ok(CompiledOperationKind::Image(ImageSourceRef {
                source_id: image.source.clone(),
                fit: image.fit,
                corner_radius: image.corner_radius,
            }))
        }
        ClipContent::Video(video) => {
            validate_source_type(layer, clip, sources, &video.source, SourceMediaType::Video)?;
            if !video.corner_radius.is_finite() || video.corner_radius < 0.0 {
                return Err(CompileError::InvalidClip {
                    layer_id: layer.id.clone(),
                    clip_id: clip.id.clone(),
                    reason: "video corner_radius must be finite and >= 0".to_string(),
                });
            }
            let _ =
                map_source_frame(&video.pipeline, 0).map_err(|err| CompileError::InvalidClip {
                    layer_id: layer.id.clone(),
                    clip_id: clip.id.clone(),
                    reason: err.to_string(),
                })?;

            Ok(CompiledOperationKind::Video(VideoSourceRef {
                source_id: video.source.clone(),
                pipeline: video.pipeline.clone(),
                fit: video.fit,
                corner_radius: video.corner_radius,
            }))
        }
    }
}

fn validate_source_type(
    layer: &Layer,
    clip: &crate::model::Clip,
    sources: &HashMap<String, Source>,
    source_id: &str,
    expected: SourceMediaType,
) -> Result<(), CompileError> {
    let source = sources
        .get(source_id)
        .ok_or_else(|| CompileError::MissingSource(source_id.to_string()))?;

    let found = source.media_type();
    if found != expected {
        return Err(CompileError::SourceTypeMismatch {
            source_id: source_id.to_string(),
            expected,
            found,
        });
    }

    if let SourceMediaType::Video = expected {
        if matches!(source.kind, crate::model::SourceKind::Generator { media, .. } if media == SourceMediaType::Audio)
        {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "video clip cannot use audio generator source".to_string(),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        compile::{CompiledOperationKind, compile_project},
        model::{
            Canvas, Clip, ClipAnimation, ClipContent, ColorRgba, Easing, Layer, Project,
            ScalarKeyframe, Source, SourceKind, SourceMediaType, TextClip, Timeline, VideoClip,
        },
        time::Rational,
    };

    #[test]
    fn compiles_basic_project() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![Source {
                id: "video_1".to_string(),
                kind: SourceKind::File {
                    media: SourceMediaType::Video,
                    path: "video.mp4".to_string(),
                },
            }],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 1,
                clips: vec![Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Default::default(),
                    animation: Default::default(),
                    content: ClipContent::Video(VideoClip {
                        source: "video_1".to_string(),
                        pipeline: Default::default(),
                        fit: Default::default(),
                        corner_radius: 0.0,
                    }),
                }],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project(&project).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
        assert_eq!(frame_ops.len(), 1);

        let op = compiled.operation(frame_ops[0]).expect("op");
        assert!(matches!(op.kind, CompiledOperationKind::Video(_)));
        assert_eq!(op.resolve_video_source_frame(0).expect("resolve"), Some(0));
    }

    #[test]
    fn rejects_incompatible_source_type() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![Source {
                id: "image_1".to_string(),
                kind: SourceKind::File {
                    media: SourceMediaType::Image,
                    path: "image.png".to_string(),
                },
            }],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                clips: vec![Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 10,
                    opacity: 1.0,
                    transform: Default::default(),
                    animation: Default::default(),
                    content: ClipContent::Video(VideoClip {
                        source: "image_1".to_string(),
                        pipeline: Default::default(),
                        fit: Default::default(),
                        corner_radius: 0.0,
                    }),
                }],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(err.to_string().contains("incompatible media type"));
    }

    #[test]
    fn rejects_out_of_range_clip() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 10,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                clips: vec![Clip {
                    id: "clip_a".to_string(),
                    start_frame: 8,
                    duration_frames: 4,
                    opacity: 1.0,
                    transform: Default::default(),
                    animation: Default::default(),
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                }],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(err.to_string().contains("beyond timeline"));
    }

    #[test]
    fn resolves_clip_animation_with_easing_and_duration() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                clips: vec![Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 0.0,
                    transform: Default::default(),
                    animation: ClipAnimation {
                        opacity: vec![ScalarKeyframe {
                            frame: 0,
                            value: 1.0,
                            duration_frames: 10,
                            easing: Easing::EaseOut,
                        }],
                        ..Default::default()
                    },
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                }],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project(&project).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(5).expect("frame ops");
        let op = compiled.operation(frame_ops[0]).expect("op");
        let midpoint = op.resolved_opacity(5);
        assert!(midpoint > 0.0 && midpoint < 1.0);
        assert_eq!(op.resolved_opacity(10), 1.0);
    }

    #[test]
    fn rejects_overlapping_animation_keyframes() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                clips: vec![Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 20,
                    opacity: 1.0,
                    transform: Default::default(),
                    animation: ClipAnimation {
                        y: vec![
                            ScalarKeyframe {
                                frame: 2,
                                value: 10.0,
                                duration_frames: 8,
                                easing: Easing::EaseOut,
                            },
                            ScalarKeyframe {
                                frame: 6,
                                value: 20.0,
                                duration_frames: 8,
                                easing: Easing::EaseIn,
                            },
                        ],
                        ..Default::default()
                    },
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                }],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(err.to_string().contains("keyframes overlap"));
    }
}
