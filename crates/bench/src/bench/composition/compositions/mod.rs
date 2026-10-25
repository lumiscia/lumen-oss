mod animated_showcase;
mod antialiasing_stress;
mod simple_pipeline;
mod small_media_transform;
mod vector_showcase;

use lumen_engine::composition::{RenderSettings, TimelineSettings};

use super::CompositionFixture;

pub fn all() -> Vec<Box<dyn CompositionFixture>> {
    vec![
        Box::new(simple_pipeline::SimplePipeline),
        Box::new(small_media_transform::SmallMediaTransform),
        Box::new(small_media_transform::SmallMediaTransformExposure),
        Box::new(vector_showcase::VectorShowcase),
        Box::new(animated_showcase::AnimatedShowcase),
        Box::new(antialiasing_stress::AntialiasingStress {
            edge_antialias: true,
        }),
        Box::new(antialiasing_stress::AntialiasingStress {
            edge_antialias: false,
        }),
    ]
}

pub fn by_name(name: &str) -> Option<Box<dyn CompositionFixture>> {
    all().into_iter().find(|fixture| fixture.name() == name)
}

pub(crate) fn timeline(fps: f32, duration_frames: u32) -> TimelineSettings {
    TimelineSettings {
        fps,
        duration_frames,
    }
}

pub(crate) fn render_720p(background_color: [u8; 4]) -> RenderSettings {
    RenderSettings {
        width: 1280,
        height: 720,
        background_color,
    }
}

pub(crate) fn render_1080p(background_color: [u8; 4]) -> RenderSettings {
    RenderSettings {
        width: 1920,
        height: 1080,
        background_color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_media_transform_fixtures_are_registered() {
        let names: Vec<_> = all().into_iter().map(|fixture| fixture.name()).collect();
        assert!(names.contains(&"small_media_transform"));
        assert!(names.contains(&"small_media_transform_exposure"));
        assert_eq!(
            by_name("small_media_transform").unwrap().name(),
            "small_media_transform"
        );
        assert_eq!(
            by_name("small_media_transform_exposure").unwrap().name(),
            "small_media_transform_exposure"
        );
    }
}
