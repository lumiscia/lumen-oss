//! Composition root model holding graph, timeline, render settings, and animation tracks.

use std::collections::HashSet;

use crate::{
    animation::KeyframeTrack,
    capability::RuntimeCapabilityProfile,
    error::{LumenError, PropertyError, Warning},
    graph::Graph,
};

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

#[derive(Debug, Clone)]
pub struct Composition {
    pub graph: Graph,
    pub timeline: TimelineSettings,
    pub render_settings: RenderSettings,
    pub tracks: Vec<KeyframeTrack>,
    pub metadata: Option<CompositionMetadata>,
}

impl Composition {
    pub fn new(graph: Graph, timeline: TimelineSettings, render_settings: RenderSettings) -> Self {
        Self {
            graph,
            timeline,
            render_settings,
            tracks: Vec::new(),
            metadata: None,
        }
    }

    pub fn add_track(&mut self, track: KeyframeTrack) {
        self.tracks.push(track)
    }

    pub fn validate(
        &self,
        profile: &RuntimeCapabilityProfile,
    ) -> Result<Vec<Warning>, Vec<LumenError>> {
        let mut warnings = Vec::new();
        let mut errors = Vec::new();

        match self.validate_structure() {
            Ok(graph_warnings) => warnings.extend(graph_warnings),
            Err(graph_errors) => errors.extend(graph_errors),
        }

        match self.validate_against_profile(profile) {
            Ok(capability_warnings) => warnings.extend(capability_warnings),
            Err(capability_errors) => errors.extend(capability_errors),
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors)
        }
    }

    pub fn validate_structure(&self) -> Result<Vec<Warning>, Vec<LumenError>> {
        let mut warnings = Vec::new();
        let mut errors = Vec::new();

        match self.graph.validate() {
            Ok(graph_warnings) => warnings.extend(graph_warnings),
            Err(graph_errors) => errors.extend(graph_errors),
        }

        for track in &self.tracks {
            if !self.graph.nodes.contains_key(&track.node_id) {
                errors.push(
                    PropertyError::MissingProperty {
                        node_id: track.node_id,
                        property_path: track.property_path.0.clone(),
                    }
                    .into(),
                );
            }
            if track.property_path.0.trim().is_empty() {
                errors.push(
                    PropertyError::MissingProperty {
                        node_id: track.node_id,
                        property_path: track.property_path.0.clone(),
                    }
                    .into(),
                );
            }

            let mut seen_frames = HashSet::new();
            for key in &track.keys {
                if !seen_frames.insert(key.time_frame) {
                    errors.push(
                        PropertyError::InvalidType {
                            node_id: track.node_id,
                            property_path: track.property_path.0.clone(),
                            expected: "unique frame times",
                            actual: "duplicate frame time",
                        }
                        .into(),
                    );
                }
            }
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors)
        }
    }
}
