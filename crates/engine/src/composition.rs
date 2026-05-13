use crate::{audio::AudioTimeline, error::LumenError, graph::Graph};

#[derive(Debug, Clone)]
pub struct TimelineSettings {
    pub fps: f32,
    pub duration_frames: u32,
}

impl TimelineSettings {
    pub fn time_seconds(&self, frame: u32) -> f64 {
        if self.fps <= 0.0 {
            return 0.0;
        }
        frame as f64 / self.fps as f64
    }
}

#[derive(Debug, Clone)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    pub background_color: [u8; 4],
}

#[derive(Debug, Clone, Default)]
pub struct CompositionMetadata {
    pub name: Option<String>,
}

#[derive(Debug)]
pub struct Composition {
    pub graph: Graph,
    pub timeline: TimelineSettings,
    pub render_settings: RenderSettings,
    pub metadata: Option<CompositionMetadata>,
    pub audio: Option<AudioTimeline>,
}

unsafe impl Sync for Composition {}
unsafe impl Send for Composition {}

impl Composition {
    pub fn new(graph: Graph, timeline: TimelineSettings, render_settings: RenderSettings) -> Self {
        Self {
            graph,
            timeline,
            render_settings,
            metadata: None,
            audio: None,
        }
    }

    pub fn validate_structure(&self) -> Result<(), Vec<LumenError>> {
        let mut errors = Vec::new();

        match self.graph.validate() {
            Ok(_) => {}
            Err(graph_errors) => errors.extend(graph_errors),
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}
