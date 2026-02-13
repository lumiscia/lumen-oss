use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, anyhow};
use lumen::{
    AudioMix, Clip, ClipAnimation, ClipContent, ColorRgba, Easing, FitMode, ImageClip, Layer,
    Project, ScalarKeyframe, Shape, ShapeClip, Source, SourceKind, SourceMediaType, SourcePipeline,
    TextAlign, TextClip, Timeline, Transform, VideoClip, time::Rational,
};
use serde::Deserialize;
use skrifa::{
    MetadataProvider,
    raw::{FileRef, FontRef},
};

const EMBEDDED_FONT: &[u8] = include_bytes!("../../../lumen/assets/roboto/Roboto-Regular.ttf");

#[derive(Debug, Clone, Deserialize)]
pub struct ChatStoryPresetV1 {
    pub kind: String,
    pub version: u32,
    #[serde(default)]
    pub canvas: ChatStoryCanvas,
    #[serde(default = "default_fps")]
    pub fps: u32,
    pub duration_seconds: f64,
    pub background: ChatStoryBackground,
    pub header: ChatStoryHeader,
    pub presentation: ChatStoryPresentation,
    #[serde(default)]
    pub messages: Vec<ChatStoryMessage>,
    #[serde(default)]
    pub overlays: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatStoryCanvas {
    #[serde(default = "default_canvas_width")]
    pub width: u32,
    #[serde(default = "default_canvas_height")]
    pub height: u32,
}

impl Default for ChatStoryCanvas {
    fn default() -> Self {
        Self {
            width: default_canvas_width(),
            height: default_canvas_height(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatStoryBackground {
    pub source: String,
    #[serde(default)]
    pub fit: FitMode,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatStoryHeader {
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub avatar_image_source: Option<String>,
    #[serde(default = "default_true")]
    pub show_back_icon: bool,
    #[serde(default = "default_true")]
    pub show_video_icon: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatStoryPresentation {
    pub expand_at_seconds: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatStoryMessage {
    pub at_seconds: f64,
    pub side: ChatStorySide,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub image_source: Option<String>,
    #[serde(default)]
    pub bubble_color: Option<[u8; 4]>,
    #[serde(default)]
    pub text_color: Option<[u8; 4]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatStorySide {
    Left,
    Right,
}

#[derive(Debug, Clone)]
struct PreparedMessage {
    index: usize,
    reveal_frame: u64,
    side: ChatStorySide,
    bubble_color: Option<ColorRgba>,
    text_color: Option<ColorRgba>,
    body: PreparedMessageBody,
}

#[derive(Debug, Clone)]
enum PreparedMessageBody {
    Text(String),
    Image { source_id: String },
}

#[derive(Debug, Clone)]
struct LayoutConstants {
    scale: f32,
    panel_width: f32,
    panel_y: f32,
    compact_height: f32,
    expanded_height: f32,
    header_height: f32,
    bubble_max_width: f32,
    bubble_radius: f32,
    text_size: f32,
    line_height: f32,
    message_gap: f32,
    panel_side_padding: f32,
    panel_top_padding: f32,
    panel_bottom_padding: f32,
    bubble_padding_x: f32,
    bubble_padding_y: f32,
    image_inset: f32,
    panel_expand_duration_frames: u64,
    message_intro_duration_frames: u64,
    message_reflow_duration_frames: u64,
    message_entry_offset_x: f32,
    message_entry_offset_y: f32,
}

#[derive(Debug, Clone)]
struct MeasuredMessage {
    index: usize,
    reveal_frame: u64,
    side: ChatStorySide,
    bubble_color: ColorRgba,
    text_color: ColorRgba,
    bubble_width: f32,
    bubble_height: f32,
    content: MeasuredMessageContent,
}

#[derive(Debug, Clone)]
enum MeasuredMessageContent {
    Text {
        lines: Vec<String>,
    },
    Image {
        source_id: String,
        draw_width: f32,
        draw_height: f32,
    },
}

#[derive(Debug, Clone)]
struct PlacedMessage {
    measured: MeasuredMessage,
    x: f32,
    y: f32,
}

#[derive(Debug, Clone)]
struct PlacementMotion {
    start_x: f32,
    start_y: f32,
    start_opacity: f32,
    animation: ClipAnimation,
}

pub fn compile_chat_story_project(preset: &ChatStoryPresetV1) -> anyhow::Result<Project> {
    validate_preset(preset)?;

    let fps_rational = Rational::new(preset.fps, 1)
        .map_err(|err| anyhow!("invalid fps `{}`: {err}", preset.fps))?;
    let total_frames = seconds_to_total_frames(preset.duration_seconds, preset.fps)?;
    let expand_at_frame =
        seconds_to_frame_floor(preset.presentation.expand_at_seconds, preset.fps)?
            .min(total_frames);
    let layout = LayoutConstants::new(preset.canvas.width, preset.fps);
    let text_measurer = TextMeasurer::new()?;

    let mut source_builder = SourceBuilder::default();
    let background_source_id = source_builder.add_file_source(
        "chat_bg",
        SourceMediaType::Video,
        preset.background.source.as_str(),
    );
    let avatar_source_id =
        preset.header.avatar_image_source.as_deref().map(|path| {
            source_builder.add_file_source("chat_avatar", SourceMediaType::Image, path)
        });

    let prepared_messages = prepare_messages(preset, total_frames, &mut source_builder)?;
    let event_frames = collect_event_frames(&prepared_messages, expand_at_frame, total_frames);

    let mut layers = Vec::new();
    layers.push(build_background_layer(
        total_frames,
        preset.canvas.width,
        preset.canvas.height,
        background_source_id,
        preset.background.fit,
    ));
    layers.push(build_chat_layer(
        preset,
        &layout,
        &text_measurer,
        &prepared_messages,
        &event_frames,
        expand_at_frame,
        avatar_source_id,
        total_frames,
    )?);

    Ok(Project {
        canvas: lumen::Canvas {
            width: preset.canvas.width,
            height: preset.canvas.height,
            background: ColorRgba(0, 0, 0, 255),
        },
        timeline: Timeline {
            fps: fps_rational,
            total_frames,
        },
        sources: source_builder.into_sources(),
        layers,
        audio: AudioMix::default(),
    })
}

fn validate_preset(preset: &ChatStoryPresetV1) -> anyhow::Result<()> {
    if preset.kind != "chat_story_v1" {
        return Err(anyhow!(
            "preset kind must be `chat_story_v1`, found `{}`",
            preset.kind
        ));
    }
    if preset.version != 1 {
        return Err(anyhow!(
            "unsupported chat_story_v1 version `{}`",
            preset.version
        ));
    }
    if preset.fps == 0 {
        return Err(anyhow!("fps must be greater than 0"));
    }
    if !preset.duration_seconds.is_finite() || preset.duration_seconds <= 0.0 {
        return Err(anyhow!("duration_seconds must be finite and > 0"));
    }
    if preset.canvas.width == 0 || preset.canvas.height == 0 {
        return Err(anyhow!("canvas width and height must be greater than 0"));
    }
    if !preset.presentation.expand_at_seconds.is_finite()
        || preset.presentation.expand_at_seconds < 0.0
    {
        return Err(anyhow!(
            "presentation.expand_at_seconds must be finite and >= 0"
        ));
    }
    if preset.presentation.expand_at_seconds > preset.duration_seconds {
        return Err(anyhow!(
            "presentation.expand_at_seconds must be <= duration_seconds"
        ));
    }
    if preset.background.source.trim().is_empty() {
        return Err(anyhow!("background.source must not be empty"));
    }
    if preset.header.title.trim().is_empty() {
        return Err(anyhow!("header.title must not be empty"));
    }

    for (index, message) in preset.messages.iter().enumerate() {
        if !message.at_seconds.is_finite() || message.at_seconds < 0.0 {
            return Err(anyhow!(
                "messages[{index}].at_seconds must be finite and >= 0"
            ));
        }
        if message.at_seconds > preset.duration_seconds {
            return Err(anyhow!(
                "messages[{index}].at_seconds must be <= duration_seconds"
            ));
        }
        let has_text = message
            .text
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        let has_image = message
            .image_source
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        if has_text == has_image {
            return Err(anyhow!(
                "messages[{index}] must include exactly one of `text` or `image_source`"
            ));
        }
    }

    Ok(())
}

fn prepare_messages(
    preset: &ChatStoryPresetV1,
    total_frames: u64,
    source_builder: &mut SourceBuilder,
) -> anyhow::Result<Vec<PreparedMessage>> {
    let mut prepared = Vec::with_capacity(preset.messages.len());

    for (index, message) in preset.messages.iter().enumerate() {
        let reveal_frame =
            seconds_to_frame_floor(message.at_seconds, preset.fps)?.min(total_frames);

        let body = if let Some(text) = message.text.as_deref() {
            PreparedMessageBody::Text(text.to_string())
        } else if let Some(image_source) = message.image_source.as_deref() {
            let source_id = source_builder.add_file_source(
                "chat_msg_image",
                SourceMediaType::Image,
                image_source,
            );
            PreparedMessageBody::Image { source_id }
        } else {
            return Err(anyhow!(
                "messages[{index}] must include exactly one of `text` or `image_source`"
            ));
        };

        prepared.push(PreparedMessage {
            index,
            reveal_frame,
            side: message.side,
            bubble_color: message.bubble_color.map(color_from_tuple),
            text_color: message.text_color.map(color_from_tuple),
            body,
        });
    }

    prepared.sort_by(|left, right| {
        left.reveal_frame
            .cmp(&right.reveal_frame)
            .then_with(|| left.index.cmp(&right.index))
    });

    Ok(prepared)
}

fn collect_event_frames(
    messages: &[PreparedMessage],
    expand_at_frame: u64,
    total_frames: u64,
) -> Vec<u64> {
    let mut events = BTreeSet::new();
    events.insert(0);
    events.insert(total_frames);
    events.insert(expand_at_frame);
    for message in messages {
        events.insert(message.reveal_frame.min(total_frames));
    }
    events.into_iter().collect()
}

fn build_background_layer(
    total_frames: u64,
    canvas_width: u32,
    canvas_height: u32,
    source_id: String,
    fit: FitMode,
) -> Layer {
    Layer {
        id: "chat_background_layer".to_string(),
        z_index: 0,
        clips: vec![Clip {
            id: "chat_background_clip".to_string(),
            start_frame: 0,
            duration_frames: total_frames,
            opacity: 1.0,
            transform: Transform {
                x: 0.0,
                y: 0.0,
                width: Some(canvas_width as f32),
                height: Some(canvas_height as f32),
                rotation_degrees: 0.0,
            },
            animation: ClipAnimation::default(),
            content: ClipContent::Video(VideoClip {
                source: source_id,
                pipeline: SourcePipeline::default(),
                fit,
                corner_radius: 0.0,
            }),
        }],
    }
}

fn build_chat_layer(
    preset: &ChatStoryPresetV1,
    layout: &LayoutConstants,
    text_measurer: &TextMeasurer,
    messages: &[PreparedMessage],
    event_frames: &[u64],
    expand_at_frame: u64,
    avatar_source_id: Option<String>,
    total_frames: u64,
) -> anyhow::Result<Layer> {
    let mut clips = Vec::new();
    let _ = preset.overlays.len();
    let panel_x = ((preset.canvas.width as f32) - layout.panel_width) * 0.5;
    let mut previous_placements: HashMap<usize, PlacedMessage> = HashMap::new();

    for (interval_index, window) in event_frames.windows(2).enumerate() {
        let start_frame = window[0];
        let end_frame = window[1];
        if end_frame <= start_frame {
            continue;
        }

        let duration_frames = end_frame - start_frame;
        let expanded = start_frame >= expand_at_frame;
        let panel_height = if expanded {
            layout.expanded_height
        } else {
            layout.compact_height
        };

        let panel_id = format!("chat_panel_i{interval_index}");
        let mut panel_clip = shape_clip(
            panel_id,
            start_frame,
            duration_frames,
            Transform {
                x: panel_x,
                y: layout.panel_y,
                width: Some(layout.panel_width),
                height: Some(panel_height),
                rotation_degrees: 0.0,
            },
            ColorRgba(10, 10, 12, 228),
            layout.scale * 2.0,
        );
        let is_expand_transition = expanded && start_frame == expand_at_frame;
        if is_expand_transition {
            let expand_frames = layout.panel_expand_duration_frames.min(duration_frames);
            if expand_frames > 0 {
                panel_clip.transform.height = Some(layout.compact_height);
                panel_clip.animation.height.push(ScalarKeyframe {
                    frame: 0,
                    value: panel_height,
                    duration_frames: expand_frames,
                    easing: Easing::EaseInOut,
                });
            }
        }
        clips.push(panel_clip);

        let header_id = format!("chat_header_i{interval_index}");
        clips.push(shape_clip(
            header_id,
            start_frame,
            duration_frames,
            Transform {
                x: panel_x,
                y: layout.panel_y,
                width: Some(layout.panel_width),
                height: Some(layout.header_height),
                rotation_degrees: 0.0,
            },
            ColorRgba(18, 18, 21, 240),
            layout.scale * 2.0,
        ));

        let avatar_size = 16.0 * layout.scale;
        let avatar_x = panel_x + (layout.panel_width * 0.5) - (avatar_size * 0.5);
        let avatar_y = layout.panel_y + (2.0 * layout.scale);
        if let Some(source_id) = avatar_source_id.as_deref() {
            clips.push(image_clip(
                format!("chat_avatar_i{interval_index}"),
                start_frame,
                duration_frames,
                Transform {
                    x: avatar_x,
                    y: avatar_y,
                    width: Some(avatar_size),
                    height: Some(avatar_size),
                    rotation_degrees: 0.0,
                },
                source_id.to_string(),
                FitMode::Cover,
                avatar_size * 0.5,
            ));
        } else {
            clips.push(shape_clip(
                format!("chat_avatar_placeholder_i{interval_index}"),
                start_frame,
                duration_frames,
                Transform {
                    x: avatar_x,
                    y: avatar_y,
                    width: Some(avatar_size),
                    height: Some(avatar_size),
                    rotation_degrees: 0.0,
                },
                ColorRgba(233, 204, 163, 255),
                avatar_size * 0.5,
            ));
        }

        if preset.header.show_back_icon {
            clips.push(text_clip(
                format!("chat_back_icon_i{interval_index}"),
                start_frame,
                duration_frames,
                Transform {
                    x: panel_x + (7.0 * layout.scale),
                    y: layout.panel_y + (8.0 * layout.scale),
                    width: Some(22.0 * layout.scale),
                    height: Some(layout.header_height),
                    rotation_degrees: 0.0,
                },
                "<".to_string(),
                9.0 * layout.scale,
                ColorRgba(72, 162, 255, 255),
                TextAlign::Left,
            ));
        }

        if preset.header.show_video_icon {
            let icon_body_x = panel_x + layout.panel_width - (15.5 * layout.scale);
            let icon_body_y = layout.panel_y + (10.5 * layout.scale);
            let icon_body_w = 8.0 * layout.scale;
            let icon_body_h = 5.0 * layout.scale;
            let icon_lens_w = 2.4 * layout.scale;
            let icon_lens_h = 2.4 * layout.scale;
            let icon_lens_x = icon_body_x + icon_body_w + (0.7 * layout.scale);
            let icon_lens_y = icon_body_y + ((icon_body_h - icon_lens_h) * 0.5);

            clips.push(shape_clip(
                format!("chat_video_icon_body_i{interval_index}"),
                start_frame,
                duration_frames,
                Transform {
                    x: icon_body_x,
                    y: icon_body_y,
                    width: Some(icon_body_w),
                    height: Some(icon_body_h),
                    rotation_degrees: 0.0,
                },
                ColorRgba(72, 162, 255, 255),
                1.2 * layout.scale,
            ));

            clips.push(shape_clip(
                format!("chat_video_icon_lens_i{interval_index}"),
                start_frame,
                duration_frames,
                Transform {
                    x: icon_lens_x,
                    y: icon_lens_y,
                    width: Some(icon_lens_w),
                    height: Some(icon_lens_h),
                    rotation_degrees: 0.0,
                },
                ColorRgba(72, 162, 255, 255),
                0.5 * layout.scale,
            ));
        }

        clips.push(text_clip(
            format!("chat_title_i{interval_index}"),
            start_frame,
            duration_frames,
            Transform {
                x: panel_x + (44.0 * layout.scale),
                y: layout.panel_y + (17.0 * layout.scale),
                width: Some(layout.panel_width - (88.0 * layout.scale)),
                height: Some(layout.header_height),
                rotation_degrees: 0.0,
            },
            preset.header.title.clone(),
            8.0 * layout.scale,
            ColorRgba(248, 248, 248, 255),
            TextAlign::Center,
        ));

        let mut visible: Vec<&PreparedMessage> = messages
            .iter()
            .filter(|message| {
                message.reveal_frame <= start_frame && message.reveal_frame < total_frames
            })
            .collect();

        if !expanded && visible.len() > 1 {
            if let Some(last) = visible.last().copied() {
                visible.clear();
                visible.push(last);
            }
        }

        let measured = visible
            .iter()
            .map(|message| measure_message(message, layout, text_measurer))
            .collect::<anyhow::Result<Vec<_>>>()?;

        let message_area_top = layout.panel_y + layout.header_height + layout.panel_top_padding;
        let message_area_bottom = layout.panel_y + panel_height - layout.panel_bottom_padding;
        let placed_messages = place_messages(
            &measured,
            panel_x,
            layout.panel_width,
            message_area_top,
            message_area_bottom,
            layout,
        );

        for placed in placed_messages.iter().cloned() {
            let motion = compute_message_motion(
                &placed,
                previous_placements.get(&placed.measured.index),
                start_frame,
                is_expand_transition,
                duration_frames,
                layout,
            );
            let bubble_clip_id = format!(
                "chat_msg_bubble_i{interval_index}_m{}",
                placed.measured.index
            );
            let mut bubble_clip = shape_clip(
                bubble_clip_id,
                start_frame,
                duration_frames,
                Transform {
                    x: motion.start_x,
                    y: motion.start_y,
                    width: Some(placed.measured.bubble_width),
                    height: Some(placed.measured.bubble_height),
                    rotation_degrees: 0.0,
                },
                placed.measured.bubble_color,
                layout.bubble_radius,
            );
            bubble_clip.opacity = motion.start_opacity;
            bubble_clip.animation = motion.animation.clone();
            clips.push(bubble_clip);

            match &placed.measured.content {
                MeasuredMessageContent::Text { lines } => {
                    for (line_index, line) in lines.iter().enumerate() {
                        let mut text = text_clip(
                            format!(
                                "chat_msg_text_i{interval_index}_m{}_l{line_index}",
                                placed.measured.index
                            ),
                            start_frame,
                            duration_frames,
                            Transform {
                                x: motion.start_x + layout.bubble_padding_x,
                                y: motion.start_y
                                    + layout.bubble_padding_y
                                    + (line_index as f32 * layout.line_height),
                                width: Some(
                                    placed.measured.bubble_width - (2.0 * layout.bubble_padding_x),
                                ),
                                height: Some(layout.line_height),
                                rotation_degrees: 0.0,
                            },
                            line.clone(),
                            layout.text_size,
                            placed.measured.text_color,
                            TextAlign::Left,
                        );
                        text.opacity = motion.start_opacity;
                        text.animation = offset_animation(
                            &motion.animation,
                            layout.bubble_padding_x,
                            layout.bubble_padding_y + (line_index as f32 * layout.line_height),
                        );
                        clips.push(text);
                    }
                }
                MeasuredMessageContent::Image {
                    source_id,
                    draw_width,
                    draw_height,
                } => {
                    let mut image = image_clip(
                        format!(
                            "chat_msg_image_i{interval_index}_m{}",
                            placed.measured.index
                        ),
                        start_frame,
                        duration_frames,
                        Transform {
                            x: motion.start_x + layout.image_inset,
                            y: motion.start_y + layout.image_inset,
                            width: Some(*draw_width),
                            height: Some(*draw_height),
                            rotation_degrees: 0.0,
                        },
                        source_id.clone(),
                        FitMode::Contain,
                        (layout.bubble_radius - layout.image_inset).max(0.0),
                    );
                    image.opacity = motion.start_opacity;
                    image.animation =
                        offset_animation(&motion.animation, layout.image_inset, layout.image_inset);
                    clips.push(image);
                }
            }
        }

        previous_placements = placed_messages
            .into_iter()
            .map(|placed| (placed.measured.index, placed))
            .collect();
    }

    Ok(Layer {
        id: "chat_overlay_layer".to_string(),
        z_index: 100,
        clips,
    })
}

fn compute_message_motion(
    placed: &PlacedMessage,
    previous: Option<&PlacedMessage>,
    start_frame: u64,
    is_expand_transition: bool,
    interval_duration: u64,
    layout: &LayoutConstants,
) -> PlacementMotion {
    let mut motion = PlacementMotion {
        start_x: placed.x,
        start_y: placed.y,
        start_opacity: 1.0,
        animation: ClipAnimation::default(),
    };

    if let Some(previous) = previous {
        let move_duration = layout.message_reflow_duration_frames.min(interval_duration);
        if (previous.x - placed.x).abs() > 0.01 {
            motion.start_x = previous.x;
            motion.animation.x.push(ScalarKeyframe {
                frame: 0,
                value: placed.x,
                duration_frames: move_duration,
                easing: Easing::EaseInOut,
            });
        }
        if (previous.y - placed.y).abs() > 0.01 {
            motion.start_y = previous.y;
            motion.animation.y.push(ScalarKeyframe {
                frame: 0,
                value: placed.y,
                duration_frames: move_duration,
                easing: Easing::EaseInOut,
            });
        }
        return motion;
    }

    let intro_duration = layout.message_intro_duration_frames.min(interval_duration);
    let is_hidden_history_message = placed.measured.reveal_frame < start_frame;
    if is_hidden_history_message {
        motion.start_opacity = 0.0;
        let delay = if is_expand_transition {
            layout
                .panel_expand_duration_frames
                .min(interval_duration.saturating_sub(1))
        } else {
            0
        };
        let fade_duration = layout
            .message_intro_duration_frames
            .min(interval_duration.saturating_sub(delay));
        motion.animation.opacity.push(ScalarKeyframe {
            frame: delay,
            value: 1.0,
            duration_frames: fade_duration,
            easing: Easing::EaseOut,
        });
        return motion;
    }

    let x_offset = match placed.measured.side {
        ChatStorySide::Left => -layout.message_entry_offset_x,
        ChatStorySide::Right => layout.message_entry_offset_x,
    };
    motion.start_x = placed.x + x_offset;
    motion.start_y = placed.y + layout.message_entry_offset_y;
    motion.animation.x.push(ScalarKeyframe {
        frame: 0,
        value: placed.x,
        duration_frames: intro_duration,
        easing: Easing::EaseOut,
    });
    motion.animation.y.push(ScalarKeyframe {
        frame: 0,
        value: placed.y,
        duration_frames: intro_duration,
        easing: Easing::EaseOut,
    });

    motion
}

fn offset_animation(animation: &ClipAnimation, offset_x: f32, offset_y: f32) -> ClipAnimation {
    let mut adjusted = animation.clone();
    for keyframe in &mut adjusted.x {
        keyframe.value += offset_x;
    }
    for keyframe in &mut adjusted.y {
        keyframe.value += offset_y;
    }
    adjusted
}

fn place_messages(
    measured: &[MeasuredMessage],
    panel_x: f32,
    panel_width: f32,
    area_top: f32,
    area_bottom: f32,
    layout: &LayoutConstants,
) -> Vec<PlacedMessage> {
    let mut y_cursor = area_bottom;
    let mut placed_reversed = Vec::new();

    for message in measured.iter().rev() {
        y_cursor -= message.bubble_height;
        if y_cursor < area_top {
            break;
        }

        let x = match message.side {
            ChatStorySide::Left => panel_x + layout.panel_side_padding,
            ChatStorySide::Right => {
                panel_x + panel_width - layout.panel_side_padding - message.bubble_width
            }
        };

        placed_reversed.push(PlacedMessage {
            measured: message.clone(),
            x,
            y: y_cursor,
        });
        y_cursor -= layout.message_gap;
    }

    placed_reversed.reverse();
    placed_reversed
}

fn measure_message(
    message: &PreparedMessage,
    layout: &LayoutConstants,
    text_measurer: &TextMeasurer,
) -> anyhow::Result<MeasuredMessage> {
    let max_bubble_width = layout.bubble_max_width;
    let max_content_width = (max_bubble_width - (2.0 * layout.bubble_padding_x)).max(1.0);

    let (content, bubble_width, bubble_height) = match &message.body {
        PreparedMessageBody::Text(text) => {
            let lines = wrap_text_greedy(text, max_content_width, layout.text_size, text_measurer)?;
            let max_line_width = lines
                .iter()
                .map(|line| text_measurer.measure(line, layout.text_size))
                .collect::<anyhow::Result<Vec<_>>>()?
                .into_iter()
                .fold(0.0f32, f32::max);

            let bubble_width = (max_line_width + (2.0 * layout.bubble_padding_x))
                .clamp(30.0 * layout.scale, max_bubble_width);
            let bubble_height = ((lines.len() as f32) * layout.line_height
                + (2.0 * layout.bubble_padding_y))
                .max(layout.line_height + (2.0 * layout.bubble_padding_y));

            (
                MeasuredMessageContent::Text { lines },
                bubble_width,
                bubble_height,
            )
        }
        PreparedMessageBody::Image { source_id } => {
            let image_width = max_bubble_width
                .min(120.0 * layout.scale)
                .max(44.0 * layout.scale);
            let image_height = (image_width * 0.72).max(30.0 * layout.scale);
            let bubble_width = image_width + (2.0 * layout.image_inset);
            let bubble_height = image_height + (2.0 * layout.image_inset);
            (
                MeasuredMessageContent::Image {
                    source_id: source_id.clone(),
                    draw_width: image_width,
                    draw_height: image_height,
                },
                bubble_width,
                bubble_height,
            )
        }
    };

    let uses_image = matches!(content, MeasuredMessageContent::Image { .. });
    let bubble_color = message
        .bubble_color
        .unwrap_or_else(|| default_bubble_color(message.side, uses_image));
    let text_color = message
        .text_color
        .unwrap_or_else(|| default_text_color(message.side, uses_image));

    Ok(MeasuredMessage {
        index: message.index,
        reveal_frame: message.reveal_frame,
        side: message.side,
        bubble_color,
        text_color,
        bubble_width,
        bubble_height,
        content,
    })
}

fn wrap_text_greedy(
    text: &str,
    max_width: f32,
    font_size: f32,
    text_measurer: &TextMeasurer,
) -> anyhow::Result<Vec<String>> {
    let mut wrapped = Vec::new();
    for paragraph in text.split('\n') {
        wrapped.extend(wrap_paragraph(
            paragraph,
            max_width,
            font_size,
            text_measurer,
        )?);
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    Ok(wrapped)
}

fn wrap_paragraph(
    paragraph: &str,
    max_width: f32,
    font_size: f32,
    text_measurer: &TextMeasurer,
) -> anyhow::Result<Vec<String>> {
    if paragraph.is_empty() {
        return Ok(vec![String::new()]);
    }

    let words: Vec<&str> = paragraph.split_whitespace().collect();
    if words.is_empty() {
        return Ok(vec![String::new()]);
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in words {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };

        let candidate_width = text_measurer.measure(candidate.as_str(), font_size)?;
        if candidate_width <= max_width {
            current = candidate;
            continue;
        }

        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }

        let word_width = text_measurer.measure(word, font_size)?;
        if word_width <= max_width {
            current = word.to_string();
            continue;
        }

        for part in hard_break_token(word, max_width, font_size, text_measurer)? {
            if !part.is_empty() {
                lines.push(part);
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    Ok(lines)
}

fn hard_break_token(
    token: &str,
    max_width: f32,
    font_size: f32,
    text_measurer: &TextMeasurer,
) -> anyhow::Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();

    for ch in token.chars() {
        let mut candidate = current.clone();
        candidate.push(ch);
        let width = text_measurer.measure(candidate.as_str(), font_size)?;
        if !current.is_empty() && width > max_width {
            parts.push(std::mem::take(&mut current));
            current.push(ch);
            continue;
        }
        if current.is_empty() && width > max_width {
            parts.push(ch.to_string());
            continue;
        }
        current.push(ch);
    }

    if !current.is_empty() {
        parts.push(current);
    }
    if parts.is_empty() {
        parts.push(token.to_string());
    }
    Ok(parts)
}

fn default_bubble_color(side: ChatStorySide, uses_image: bool) -> ColorRgba {
    match (side, uses_image) {
        (ChatStorySide::Left, _) => ColorRgba(42, 42, 45, 255),
        (ChatStorySide::Right, true) => ColorRgba(245, 245, 246, 255),
        (ChatStorySide::Right, false) => ColorRgba(53, 150, 255, 255),
    }
}

fn default_text_color(side: ChatStorySide, uses_image: bool) -> ColorRgba {
    match (side, uses_image) {
        (ChatStorySide::Right, true) => ColorRgba(24, 24, 26, 255),
        _ => ColorRgba(255, 255, 255, 255),
    }
}

fn shape_clip(
    id: String,
    start_frame: u64,
    duration_frames: u64,
    transform: Transform,
    fill: ColorRgba,
    radius: f32,
) -> Clip {
    Clip {
        id,
        start_frame,
        duration_frames,
        opacity: 1.0,
        transform,
        animation: ClipAnimation::default(),
        content: ClipContent::Shape(ShapeClip {
            shape: Shape::Rectangle { fill, radius },
        }),
    }
}

fn text_clip(
    id: String,
    start_frame: u64,
    duration_frames: u64,
    transform: Transform,
    text: String,
    font_size: f32,
    color: ColorRgba,
    align: TextAlign,
) -> Clip {
    Clip {
        id,
        start_frame,
        duration_frames,
        opacity: 1.0,
        transform,
        animation: ClipAnimation::default(),
        content: ClipContent::Text(TextClip {
            text,
            font_size,
            color,
            align,
        }),
    }
}

fn image_clip(
    id: String,
    start_frame: u64,
    duration_frames: u64,
    transform: Transform,
    source: String,
    fit: FitMode,
    corner_radius: f32,
) -> Clip {
    Clip {
        id,
        start_frame,
        duration_frames,
        opacity: 1.0,
        transform,
        animation: ClipAnimation::default(),
        content: ClipContent::Image(ImageClip {
            source,
            fit,
            corner_radius,
        }),
    }
}

fn color_from_tuple(value: [u8; 4]) -> ColorRgba {
    ColorRgba(value[0], value[1], value[2], value[3])
}

fn seconds_to_total_frames(duration_seconds: f64, fps: u32) -> anyhow::Result<u64> {
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return Err(anyhow!("duration_seconds must be finite and > 0"));
    }
    let raw = (duration_seconds * fps as f64).ceil().max(1.0);
    if raw > u64::MAX as f64 {
        return Err(anyhow!("duration_seconds produced too many frames"));
    }
    Ok(raw as u64)
}

fn seconds_to_frame_floor(seconds: f64, fps: u32) -> anyhow::Result<u64> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(anyhow!("time values must be finite and >= 0"));
    }
    let raw = (seconds * fps as f64).floor().max(0.0);
    if raw > u64::MAX as f64 {
        return Err(anyhow!("time value produced too many frames"));
    }
    Ok(raw as u64)
}

fn seconds_to_frames(seconds: f64, fps: u32) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 || fps == 0 {
        return 0;
    }
    let raw = (seconds * fps as f64).round().max(0.0);
    raw.min(u64::MAX as f64) as u64
}

fn default_canvas_width() -> u32 {
    1080
}

fn default_canvas_height() -> u32 {
    1920
}

fn default_fps() -> u32 {
    30
}

fn default_true() -> bool {
    true
}

impl LayoutConstants {
    fn new(canvas_width: u32, fps: u32) -> Self {
        let scale = (canvas_width as f32) / 360.0;
        let text_size = 12.0 * scale;
        let panel_expand_duration_frames = seconds_to_frames(0.24, fps).max(1);
        let message_intro_duration_frames = seconds_to_frames(0.18, fps).max(1);
        let message_reflow_duration_frames = seconds_to_frames(0.22, fps).max(1);
        Self {
            scale,
            panel_width: 248.0 * scale,
            panel_y: 132.0 * scale,
            compact_height: 88.0 * scale,
            expanded_height: 438.0 * scale,
            header_height: 28.0 * scale,
            bubble_max_width: 178.0 * scale,
            bubble_radius: 10.0 * scale,
            text_size,
            line_height: 1.28 * text_size,
            message_gap: 6.0 * scale,
            panel_side_padding: 10.0 * scale,
            panel_top_padding: 6.0 * scale,
            panel_bottom_padding: 8.0 * scale,
            bubble_padding_x: 8.0 * scale,
            bubble_padding_y: 5.0 * scale,
            image_inset: 2.0 * scale,
            panel_expand_duration_frames,
            message_intro_duration_frames,
            message_reflow_duration_frames,
            message_entry_offset_x: 12.0 * scale,
            message_entry_offset_y: 0.0,
        }
    }
}

#[derive(Default)]
struct SourceBuilder {
    sources: Vec<Source>,
    dedupe: HashMap<String, String>,
    next_id: usize,
}

impl SourceBuilder {
    fn add_file_source(&mut self, prefix: &str, media: SourceMediaType, path: &str) -> String {
        let media_label = match media {
            SourceMediaType::Video => "video",
            SourceMediaType::Image => "image",
            SourceMediaType::Audio => "audio",
        };
        let key = format!("{media_label}:{path}");
        if let Some(existing) = self.dedupe.get(key.as_str()) {
            return existing.clone();
        }

        let source_id = format!("{prefix}_{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.sources.push(Source {
            id: source_id.clone(),
            kind: SourceKind::File {
                media,
                path: path.to_string(),
            },
        });
        self.dedupe.insert(key, source_id.clone());
        source_id
    }

    fn into_sources(self) -> Vec<Source> {
        self.sources
    }
}

struct TextMeasurer {
    font: FontRef<'static>,
}

impl TextMeasurer {
    fn new() -> anyhow::Result<Self> {
        let file_ref = FileRef::new(EMBEDDED_FONT).context("failed to parse embedded font file")?;
        let font = match file_ref {
            FileRef::Font(font) => font,
            FileRef::Collection(collection) => collection
                .get(0)
                .context("embedded font collection did not include index 0")?,
        };
        Ok(Self { font })
    }

    fn measure(&self, text: &str, font_size: f32) -> anyhow::Result<f32> {
        if !font_size.is_finite() || font_size <= 0.0 {
            return Err(anyhow!("font_size must be finite and > 0"));
        }
        let axes = self.font.axes();
        let location = axes.location(std::iter::empty::<(&str, f32)>());
        let glyph_metrics = self
            .font
            .glyph_metrics(skrifa::instance::Size::new(font_size), &location);
        let charmap = self.font.charmap();

        let width = text.chars().fold(0.0f32, |acc, ch| {
            let gid = charmap.map(ch).unwrap_or_default();
            let advance = glyph_metrics.advance_width(gid).unwrap_or(font_size * 0.5);
            acc + advance
        });

        Ok(width)
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatStoryPresetV1, compile_chat_story_project};

    fn sample_preset() -> ChatStoryPresetV1 {
        serde_json::from_value(serde_json::json!({
            "kind": "chat_story_v1",
            "version": 1,
            "canvas": { "width": 1080, "height": 1920 },
            "fps": 30,
            "duration_seconds": 8,
            "background": { "source": "videos/ref.mp4", "fit": "cover" },
            "header": { "title": "UBER EATS guy(smash)" },
            "presentation": { "expand_at_seconds": 2.5 },
            "messages": [
                { "at_seconds": 0.5, "side": "left", "text": "I deliver your food sir." },
                { "at_seconds": 1.2, "side": "right", "text": "Have fun delivering my food." },
                { "at_seconds": 3.1, "side": "left", "text": "No problem sir." },
                { "at_seconds": 6.0, "side": "right", "image_source": "images/tip.jpg" }
            ]
        }))
        .expect("sample preset")
    }

    #[test]
    fn validates_message_content_shape() {
        let preset: ChatStoryPresetV1 = serde_json::from_value(serde_json::json!({
            "kind": "chat_story_v1",
            "version": 1,
            "duration_seconds": 5,
            "background": { "source": "videos/ref.mp4" },
            "header": { "title": "x" },
            "presentation": { "expand_at_seconds": 1 },
            "messages": [{ "at_seconds": 0.5, "side": "left", "text": "x", "image_source": "y.png" }]
        }))
        .expect("preset");

        let err =
            compile_chat_story_project(&preset).expect_err("must reject invalid message shape");
        assert!(
            err.to_string()
                .contains("exactly one of `text` or `image_source`")
        );
    }

    #[test]
    fn validates_expand_time_is_in_range() {
        let preset: ChatStoryPresetV1 = serde_json::from_value(serde_json::json!({
            "kind": "chat_story_v1",
            "version": 1,
            "duration_seconds": 2,
            "background": { "source": "videos/ref.mp4" },
            "header": { "title": "x" },
            "presentation": { "expand_at_seconds": 3 },
            "messages": [{ "at_seconds": 0.5, "side": "left", "text": "x" }]
        }))
        .expect("preset");

        let err =
            compile_chat_story_project(&preset).expect_err("must reject out-of-range expand time");
        assert!(err.to_string().contains("expand_at_seconds"));
    }

    #[test]
    fn compiles_deterministic_event_intervals() {
        let preset = sample_preset();
        let project_a = compile_chat_story_project(&preset).expect("compile a");
        let project_b = compile_chat_story_project(&preset).expect("compile b");
        assert_eq!(
            serde_json::to_value(&project_a).expect("serialize a"),
            serde_json::to_value(&project_b).expect("serialize b")
        );

        let chat_layer = project_a
            .layers
            .iter()
            .find(|layer| layer.id == "chat_overlay_layer")
            .expect("chat layer");
        let panel_starts: Vec<u64> = chat_layer
            .clips
            .iter()
            .filter(|clip| clip.id.starts_with("chat_panel_i"))
            .map(|clip| clip.start_frame)
            .collect();
        assert_eq!(panel_starts, vec![0, 15, 36, 75, 93, 180]);
    }

    #[test]
    fn keeps_message_bubbles_inside_panel_bounds() {
        let preset = sample_preset();
        let project = compile_chat_story_project(&preset).expect("compile");
        let chat_layer = project
            .layers
            .iter()
            .find(|layer| layer.id == "chat_overlay_layer")
            .expect("chat layer");

        let panel_x =
            (project.canvas.width as f32 - (248.0 * (project.canvas.width as f32 / 360.0))) * 0.5;
        let panel_w = 248.0 * (project.canvas.width as f32 / 360.0);

        for bubble in chat_layer
            .clips
            .iter()
            .filter(|clip| clip.id.starts_with("chat_msg_bubble_"))
        {
            let x = bubble
                .animation
                .x
                .last()
                .map(|keyframe| keyframe.value)
                .unwrap_or(bubble.transform.x);
            let width = bubble.transform.width.unwrap_or_default();
            assert!(x >= panel_x - 0.1);
            assert!(x + width <= panel_x + panel_w + 0.1);
        }
    }
}
