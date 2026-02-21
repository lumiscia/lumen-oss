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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LayoutNodeStyle {
    #[serde(default)]
    pub width: Option<StyleValue>,
    #[serde(default)]
    pub height: Option<StyleValue>,
    #[serde(default)]
    pub gap: Option<StyleValue>,
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
