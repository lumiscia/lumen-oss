use serde::{Deserialize, Serialize};

use super::StyleValue;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LayoutNode {
    pub id: String,
    #[serde(default)]
    pub style: LayoutNodeStyle,
    pub kind: LayoutNodeKind,
}

impl LayoutNode {
    pub fn children(&self) -> &[LayoutNode] {
        match &self.kind {
            LayoutNodeKind::Container { children, .. } => children.as_slice(),
            LayoutNodeKind::Text { .. } | LayoutNodeKind::Image { .. } => &[],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LayoutNodeStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding_left: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding_top: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding_right: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding_bottom: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grow: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shrink: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<StyleValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub justify: Option<LayoutJustify>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<LayoutAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<LayoutDirection>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutJustify {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutAlign {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutDirection {
    Row,
    Column,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LayoutNodeKind {
    Container {
        #[serde(default)]
        children: Vec<LayoutNode>,
    },
    Text {
        content: String,
    },
    Image {
        source: String,
    },
}
