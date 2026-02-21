use serde::{Deserialize, Serialize};

use super::{BaseStyle, ClipStyle, LayoutNode, StyleValue};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Layer {
    pub id: String,
    #[serde(default)]
    pub items: Vec<LayerItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LayerItem {
    Clip(ClipItem),
    Group(GroupItem),
}

impl LayerItem {
    pub fn id(&self) -> &str {
        match self {
            Self::Clip(clip) => clip.id.as_str(),
            Self::Group(group) => group.id.as_str(),
        }
    }

    pub fn mask(&self) -> Option<&LayerItem> {
        match self {
            Self::Clip(clip) => clip.mask.as_deref(),
            Self::Group(group) => group.mask.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ClipItem {
    pub id: String,
    pub start_frame: u64,
    pub duration_frames: u64,
    pub content: ClipContent,
    #[serde(default)]
    pub style: ClipStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<LayerItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct GroupItem {
    pub id: String,
    #[serde(default)]
    pub items: Vec<LayerItem>,
    #[serde(default)]
    pub style: BaseStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<LayerItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipContent {
    Solid,
    Shape {
        geometry: ShapeGeometry,
    },
    Text {
        content: String,
    },
    Image {
        source: String,
    },
    Video {
        source: String,
        #[serde(default)]
        pipeline: VideoPipeline,
    },
    Layout {
        root: LayoutNode,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct VideoPipeline {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<TrimRange>,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default)]
    pub r#loop: LoopMode,
}

impl Default for VideoPipeline {
    fn default() -> Self {
        Self {
            trim: None,
            speed: default_speed(),
            r#loop: LoopMode::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TrimRange {
    pub start_frame: u64,
    pub end_frame: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum LoopMode {
    Label(LoopLabel),
    Finite { finite: u32 },
}

impl Default for LoopMode {
    fn default() -> Self {
        Self::Label(LoopLabel::None)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoopLabel {
    None,
    Infinite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShapeGeometry {
    Rect,
    Ellipse,
    Polygon {
        vertices: Vec<PolygonVertex>,
        #[serde(default = "default_polygon_closed")]
        closed: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct PolygonVertex {
    pub x: StyleValue,
    pub y: StyleValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cp_in: Option<[StyleValue; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cp_out: Option<[StyleValue; 2]>,
}

impl PolygonVertex {
    pub fn from_literal(x: f32, y: f32) -> Self {
        Self {
            x: StyleValue::Value(x),
            y: StyleValue::Value(y),
            cp_in: None,
            cp_out: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SourceFrameContext {
    pub local_frame: u64,
    pub source_length: u64,
}

impl VideoPipeline {
    pub fn source_frame_for(&self, ctx: SourceFrameContext) -> Option<u64> {
        if ctx.source_length == 0 {
            return None;
        }

        let trim = self.trim.unwrap_or(TrimRange {
            start_frame: 0,
            end_frame: ctx.source_length,
        });
        let start = trim.start_frame.min(ctx.source_length);
        let end = trim.end_frame.min(ctx.source_length);
        if end <= start {
            return None;
        }
        let len = end - start;
        let speed = self.speed;
        if !speed.is_finite() {
            return None;
        }

        let source_step = ((ctx.local_frame as f64) * (speed.abs() as f64)).floor() as u64;
        let total_span = match self.r#loop {
            LoopMode::Label(LoopLabel::None) => len,
            LoopMode::Label(LoopLabel::Infinite) => len,
            LoopMode::Finite { finite } => len.saturating_mul(u64::from(finite.max(1))),
        };

        if matches!(self.r#loop, LoopMode::Label(LoopLabel::None)) && source_step >= total_span {
            return None;
        }
        if matches!(self.r#loop, LoopMode::Finite { .. }) && source_step >= total_span {
            return None;
        }

        let within_trim = if len == 0 {
            0
        } else {
            match self.r#loop {
                LoopMode::Label(LoopLabel::None) => source_step.min(len.saturating_sub(1)),
                LoopMode::Label(LoopLabel::Infinite) | LoopMode::Finite { .. } => source_step % len,
            }
        };

        let frame = if speed < 0.0 {
            end.saturating_sub(1).saturating_sub(within_trim)
        } else {
            start.saturating_add(within_trim)
        };

        Some(frame.min(ctx.source_length.saturating_sub(1)))
    }
}

fn default_speed() -> f32 {
    1.0
}

fn default_polygon_closed() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{LoopLabel, LoopMode, SourceFrameContext, TrimRange, VideoPipeline};

    #[test]
    fn source_frame_without_loop_stops_at_end() {
        let pipeline = VideoPipeline {
            trim: Some(TrimRange {
                start_frame: 10,
                end_frame: 20,
            }),
            speed: 1.0,
            r#loop: LoopMode::Label(LoopLabel::None),
        };

        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 0,
                source_length: 100
            }),
            Some(10)
        );
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 9,
                source_length: 100
            }),
            Some(19)
        );
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 10,
                source_length: 100
            }),
            None
        );
    }

    #[test]
    fn source_frame_infinite_loop_wraps() {
        let pipeline = VideoPipeline {
            trim: Some(TrimRange {
                start_frame: 4,
                end_frame: 7,
            }),
            speed: 1.0,
            r#loop: LoopMode::Label(LoopLabel::Infinite),
        };

        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 0,
                source_length: 100
            }),
            Some(4)
        );
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 3,
                source_length: 100
            }),
            Some(4)
        );
    }

    #[test]
    fn source_frame_finite_loop_stops_after_span() {
        let pipeline = VideoPipeline {
            trim: Some(TrimRange {
                start_frame: 20,
                end_frame: 23,
            }),
            speed: 1.0,
            r#loop: LoopMode::Finite { finite: 2 },
        };

        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 5,
                source_length: 100
            }),
            Some(22)
        );
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 6,
                source_length: 100
            }),
            None
        );
    }

    #[test]
    fn source_frame_negative_speed_reverses_trim_window() {
        let pipeline = VideoPipeline {
            trim: Some(TrimRange {
                start_frame: 10,
                end_frame: 15,
            }),
            speed: -1.0,
            r#loop: LoopMode::Label(LoopLabel::Infinite),
        };

        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 0,
                source_length: 100
            }),
            Some(14)
        );
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 1,
                source_length: 100
            }),
            Some(13)
        );
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 5,
                source_length: 100
            }),
            Some(14)
        );
    }

    #[test]
    fn source_frame_clamps_trim_to_source_length() {
        let pipeline = VideoPipeline {
            trim: Some(TrimRange {
                start_frame: 90,
                end_frame: 200,
            }),
            speed: 1.0,
            r#loop: LoopMode::Label(LoopLabel::None),
        };

        // source_length=100, so effective trim is 90..100 (len=10)
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 0,
                source_length: 100
            }),
            Some(90)
        );
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 9,
                source_length: 100
            }),
            Some(99)
        );
        assert_eq!(
            pipeline.source_frame_for(SourceFrameContext {
                local_frame: 10,
                source_length: 100
            }),
            None
        );
    }
}
