use crate::{
    media::MediaStore,
    node::{NodeId, NodeProperty, PortRef},
    raster::RasterFrame,
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct TimeRemap {
    pub id: NodeId,

    #[property(expected = Float)]
    pub source_frame: NodeProperty,
    #[property(expected = Float)]
    pub offset: NodeProperty,
    #[property(expected = Float)]
    pub speed: NodeProperty,
    #[property(expected = Bool)]
    pub loop_enabled: NodeProperty,
    #[property(expected = Int)]
    pub loop_start: NodeProperty,
    #[property(expected = Int)]
    pub loop_end: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for TimeRemap {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            source_frame: NodeProperty::Float(0.0),
            offset: NodeProperty::Float(0.0),
            speed: NodeProperty::Float(1.0),
            loop_enabled: NodeProperty::Bool(false),
            loop_start: NodeProperty::Int(0),
            loop_end: NodeProperty::Int(0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl TimeRemap {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let target_frame = remap_frame(
            ctx.frame,
            TimeRemapSettings {
                source_frame: self.resolve_source_frame(ctx)?,
                offset: self.resolve_offset(ctx)?,
                speed: self.resolve_speed(ctx)?,
                loop_enabled: self.resolve_loop_enabled(ctx)?,
                loop_start: self.resolve_loop_start(ctx)?,
                loop_end: self.resolve_loop_end(ctx)?,
            },
        );

        with_frame(ctx, target_frame, |ctx| {
            ctx.eval_once(&self.source)?.as_raster()?.snapshot()
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TimeRemapSettings {
    pub source_frame: f64,
    pub offset: f64,
    pub speed: f64,
    pub loop_enabled: bool,
    pub loop_start: i64,
    pub loop_end: i64,
}

pub fn remap_frame(current_frame: u32, settings: TimeRemapSettings) -> u32 {
    let mapped =
        settings.source_frame + ((f64::from(current_frame) - settings.offset) * settings.speed);
    let mapped = if settings.loop_enabled {
        wrap_frame(mapped, settings.loop_start, settings.loop_end)
    } else {
        mapped
    };

    if !mapped.is_finite() || mapped <= 0.0 {
        return 0;
    }
    mapped.round().min(f64::from(u32::MAX)) as u32
}

fn wrap_frame(frame: f64, loop_start: i64, loop_end: i64) -> f64 {
    if loop_end <= loop_start {
        return frame;
    }

    let start = loop_start as f64;
    let len = (loop_end - loop_start) as f64;
    (frame - start).rem_euclid(len) + start
}

pub fn with_frame<S: SurfacePool, M: MediaStore, T>(
    ctx: &mut RenderContext<'_, S, M>,
    frame: u32,
    f: impl FnOnce(&mut RenderContext<'_, S, M>) -> crate::Result<T>,
) -> crate::Result<T> {
    let original_frame = ctx.frame;
    ctx.frame = frame;
    let result = f(ctx);
    ctx.frame = original_frame;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        composition::{Composition, RenderSettings, TimelineSettings},
        error::{LumenError, RenderError},
        graph::Graph,
        media::{ImageResolver, MediaStore, VideoFrameResolver},
        render::{LumenRenderer, RenderContext, surface::DefaultSurfacePool},
    };

    #[derive(Debug)]
    struct NullMediaStore;

    impl MediaStore for NullMediaStore {
        fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }
    }

    #[test]
    fn maps_source_frame_offset_speed_and_loop() {
        let settings = TimeRemapSettings {
            source_frame: 10.0,
            offset: 2.0,
            speed: 2.0,
            loop_enabled: false,
            loop_start: 0,
            loop_end: 0,
        };
        assert_eq!(remap_frame(5, settings), 16);

        let looped = TimeRemapSettings {
            loop_enabled: true,
            loop_start: 12,
            loop_end: 16,
            ..settings
        };
        assert_eq!(remap_frame(6, looped), 14);
    }

    #[test]
    fn restores_context_frame_after_error() {
        let composition = Composition::new(
            Graph::new(),
            TimelineSettings {
                fps: 30.0,
                duration_frames: 60,
            },
            RenderSettings {
                width: 8,
                height: 8,
                background_color: [0, 0, 0, 0],
            },
        );
        let pool = DefaultSurfacePool::new();
        let media = NullMediaStore;
        let renderer = LumenRenderer::new(&composition, &pool, &media).unwrap();
        let mut ctx = RenderContext::new(&renderer, 3);

        let result: crate::Result<()> = with_frame(&mut ctx, 42, |ctx| {
            assert_eq!(ctx.frame, 42);
            Err(LumenError::Render(RenderError::Cancelled {
                frame: ctx.frame,
            }))
        });

        assert!(result.is_err());
        assert_eq!(ctx.frame, 3);
    }
}
