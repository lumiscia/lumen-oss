//! Runtime capability profile checks for composition execution constraints.

use crate::{
    composition::Composition,
    error::{LumenError, RenderError, Warning},
    node::NodeKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkType {
    Bitmap,
    Video,
    ImageSequence,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeCapabilityProfile {
    pub has_image_resolver: bool,
    pub has_video_resolver: bool,
    pub has_threading: bool,
    pub sink_types: Vec<SinkType>,
}

impl RuntimeCapabilityProfile {
    pub fn cpu_only() -> Self {
        Self {
            has_image_resolver: false,
            has_video_resolver: false,
            has_threading: false,
            sink_types: vec![SinkType::Bitmap],
        }
    }
}

impl Composition {
    pub fn validate_against_profile(
        &self,
        profile: &RuntimeCapabilityProfile,
    ) -> Result<Vec<Warning>, Vec<LumenError>> {
        let warnings = Vec::new();
        let mut errors = Vec::new();

        for node in self.graph.nodes.values() {
            match &node.kind {
                NodeKind::MediaIn(_) => {
                    if !profile.has_image_resolver && !profile.has_video_resolver {
                        errors.push(
                            RenderError::NodeEvaluation {
                                frame: 0,
                                node_id: node.id,
                                node_kind: node.kind.kind_name(),
                                details: "media node requires an image or video resolver"
                                    .to_string(),
                            }
                            .into(),
                        )
                    }
                }
                _ => {}
            }
        }

        if profile.sink_types.is_empty() {
            errors.push(
                RenderError::NodeEvaluation {
                    frame: 0,
                    node_id: crate::node::NodeId(0),
                    node_kind: "Composition",
                    details: "no sink types declared in runtime profile".to_string(),
                }
                .into(),
            )
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors)
        }
    }
}
