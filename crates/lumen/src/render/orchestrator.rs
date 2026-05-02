use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

use crate::{
    composition::Composition,
    gpu_image::GpuImageFrame,
    media::{FrameRequirements, MediaStore, collect_frame_requirements},
    render::{LumenRenderer, surface::SurfacePool},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOrchestratorConfig {
    pub lookahead_count: u32,
}

impl Default for RenderOrchestratorConfig {
    fn default() -> Self {
        Self { lookahead_count: 0 }
    }
}

pub struct RenderOrchestrator<'a, S: SurfacePool, M: MediaStore> {
    composition: &'a Composition,
    surface_pool: &'a S,
    media_store: &'a M,
    config: RenderOrchestratorConfig,
    retained_streams: Mutex<BTreeSet<String>>,
}

impl<'a, S: SurfacePool, M: MediaStore> RenderOrchestrator<'a, S, M> {
    pub fn new(
        composition: &'a Composition,
        surface_pool: &'a S,
        media_store: &'a M,
        config: RenderOrchestratorConfig,
    ) -> Self {
        Self {
            composition,
            surface_pool,
            media_store,
            config,
            retained_streams: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn render(&self, frame: u32) -> crate::Result<GpuImageFrame> {
        let current = collect_frame_requirements(self.composition, self.media_store, frame)?;
        let window = self.collect_window_requirements(frame)?;
        self.enqueue_window(&current)?;

        let mut renderer =
            LumenRenderer::new(self.composition, self.surface_pool, self.media_store)?;
        let output = renderer.render(frame)?;

        let future = window_without_current(window.clone(), &current);
        self.enqueue_window(&future)?;
        self.retain_window(&window);
        Ok(output)
    }

    fn collect_window_requirements(&self, frame: u32) -> crate::Result<FrameRequirements> {
        let mut window = FrameRequirements::default();
        let duration = self.composition.timeline.duration_frames;
        let last_frame = if duration == 0 {
            frame
        } else {
            frame
                .saturating_add(self.config.lookahead_count)
                .min(duration.saturating_sub(1))
        };

        for predicted_frame in frame..=last_frame {
            let requirements =
                collect_frame_requirements(self.composition, self.media_store, predicted_frame)?;
            merge_requirements(&mut window, requirements);
        }

        sort_and_dedupe(&mut window);
        Ok(window)
    }

    fn enqueue_window(&self, window: &FrameRequirements) -> crate::Result<()> {
        for video in &window.videos {
            let resolver = self
                .media_store
                .get_video_resolver(&video.stream_id)
                .ok_or_else(|| crate::error::MediaError::SourceNotFound {
                    media_source: video.stream_id.clone(),
                })?;
            for frame in &video.frames {
                resolver.enqueue_frame(*frame)?;
            }
        }

        Ok(())
    }

    fn retain_window(&self, window: &FrameRequirements) {
        let current_streams = window
            .videos
            .iter()
            .map(|video| video.stream_id.clone())
            .collect::<BTreeSet<_>>();
        let mut streams_to_retain = current_streams.clone();
        if let Ok(retained_streams) = self.retained_streams.lock() {
            streams_to_retain.extend(retained_streams.iter().cloned());
        }

        let frames_by_stream = window
            .videos
            .iter()
            .map(|video| (video.stream_id.as_str(), video.frames.as_slice()))
            .collect::<BTreeMap<_, _>>();

        for stream_id in streams_to_retain {
            if let Some(resolver) = self.media_store.get_video_resolver(&stream_id) {
                let frames = frames_by_stream
                    .get(stream_id.as_str())
                    .copied()
                    .unwrap_or(&[]);
                resolver.retain_frames(frames);
            }
        }

        if let Ok(mut retained_streams) = self.retained_streams.lock() {
            *retained_streams = current_streams;
        }
    }
}

impl<'a, S: SurfacePool, M: MediaStore> std::fmt::Debug for RenderOrchestrator<'a, S, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderOrchestrator")
            .field("composition", self.composition)
            .field("surface_pool", self.surface_pool)
            .field("media_store", self.media_store)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

fn merge_requirements(target: &mut FrameRequirements, source: FrameRequirements) {
    target.images.extend(source.images);
    target.videos.extend(source.videos);
}

fn sort_and_dedupe(requirements: &mut FrameRequirements) {
    requirements.images.sort();
    requirements.images.dedup();

    let mut videos = BTreeMap::<String, Vec<u32>>::new();
    for video in requirements.videos.drain(..) {
        videos
            .entry(video.stream_id)
            .or_default()
            .extend(video.frames);
    }

    requirements.videos = videos
        .into_iter()
        .map(|(stream_id, mut frames)| {
            frames.sort_unstable();
            frames.dedup();
            crate::media::VideoFrameRequirement { stream_id, frames }
        })
        .collect();
}

fn window_without_current(
    mut window: FrameRequirements,
    current: &FrameRequirements,
) -> FrameRequirements {
    let current_frames = current
        .videos
        .iter()
        .map(|video| (video.stream_id.as_str(), video.frames.as_slice()))
        .collect::<BTreeMap<_, _>>();

    for video in &mut window.videos {
        if let Some(frames) = current_frames.get(video.stream_id.as_str()) {
            video.frames.retain(|frame| !frames.contains(frame));
        }
    }
    window.videos.retain(|video| !video.frames.is_empty());
    window
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::{
        composition::{Composition, RenderSettings, TimelineSettings},
        error::MediaError,
        gpu_image::{AlphaMode, GpuImageFrame, RectI},
        graph::{Connection, Graph},
        media::{ImageResolver, VideoFrameResolver, VideoMetadata},
        node::{
            NodeId, NodeKind, NodeProperty, PortRef, media_output::MediaOutput,
            source::media_in::MediaIn,
        },
        render::surface::DefaultSurfacePool,
    };

    use super::*;

    #[derive(Debug, Default)]
    struct TestMediaStore {
        resolver: Arc<TestVideoResolver>,
    }

    impl MediaStore for TestMediaStore {
        fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
            Some(Box::new(SharedTestVideoResolver(Arc::clone(
                &self.resolver,
            ))))
        }
    }

    #[derive(Debug, Default)]
    struct TestVideoResolver {
        enqueued: Mutex<Vec<u32>>,
        retained: Mutex<Vec<Vec<u32>>>,
        resolved: Mutex<Vec<u32>>,
    }

    #[derive(Debug)]
    struct SharedTestVideoResolver(Arc<TestVideoResolver>);

    impl VideoFrameResolver for SharedTestVideoResolver {
        fn id(&self) -> &str {
            "video"
        }

        fn metadata(&self) -> VideoMetadata {
            VideoMetadata {
                width: 1,
                height: 1,
                frame_count: 120,
            }
        }

        fn enqueue_frame(&self, frame: u32) -> Result<(), MediaError> {
            self.0.enqueued.lock().expect("enqueued lock").push(frame);
            Ok(())
        }

        fn frame(&self, frame: u32) -> Result<Arc<GpuImageFrame>, MediaError> {
            self.0.resolved.lock().expect("resolved lock").push(frame);
            Ok(Arc::new(test_frame()))
        }

        fn retain_frames(&self, frames: &[u32]) {
            self.0
                .retained
                .lock()
                .expect("retained lock")
                .push(frames.to_vec());
        }
    }

    #[test]
    fn render_enqueues_current_frame_and_lookahead_window() {
        let store = TestMediaStore::default();
        let composition = video_composition();
        let pool = DefaultSurfacePool::new();
        let orchestrator = RenderOrchestrator::new(
            &composition,
            &pool,
            &store,
            RenderOrchestratorConfig { lookahead_count: 3 },
        );

        orchestrator.render(4).expect("render frame");

        assert_eq!(
            store
                .resolver
                .enqueued
                .lock()
                .expect("enqueued lock")
                .as_slice(),
            &[4, 5, 6, 7]
        );
        assert_eq!(
            store
                .resolver
                .resolved
                .lock()
                .expect("resolved lock")
                .as_slice(),
            &[4]
        );
        assert_eq!(
            store
                .resolver
                .retained
                .lock()
                .expect("retained lock")
                .last()
                .expect("retain call")
                .as_slice(),
            &[4, 5, 6, 7]
        );
    }

    #[test]
    fn rolling_windows_retain_only_the_current_prediction_window() {
        let store = TestMediaStore::default();
        let composition = video_composition();
        let pool = DefaultSurfacePool::new();
        let orchestrator = RenderOrchestrator::new(
            &composition,
            &pool,
            &store,
            RenderOrchestratorConfig { lookahead_count: 2 },
        );

        orchestrator.render(0).expect("render frame 0");
        orchestrator.render(1).expect("render frame 1");

        let retained = store.resolver.retained.lock().expect("retained lock");
        assert_eq!(retained[0].as_slice(), &[0, 1, 2]);
        assert_eq!(retained[1].as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn render_succeeds_with_resolver_that_does_not_enqueue() {
        let store = NoEnqueueMediaStore;
        let composition = video_composition();
        let pool = DefaultSurfacePool::new();
        let orchestrator = RenderOrchestrator::new(
            &composition,
            &pool,
            &store,
            RenderOrchestratorConfig { lookahead_count: 1 },
        );

        orchestrator.render(0).expect("render frame");
    }

    #[derive(Debug)]
    struct NoEnqueueMediaStore;

    impl MediaStore for NoEnqueueMediaStore {
        fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
            Some(Box::new(NoEnqueueResolver))
        }
    }

    struct NoEnqueueResolver;

    impl VideoFrameResolver for NoEnqueueResolver {
        fn id(&self) -> &str {
            "video"
        }

        fn metadata(&self) -> VideoMetadata {
            VideoMetadata {
                width: 1,
                height: 1,
                frame_count: 120,
            }
        }

        fn frame(&self, _frame: u32) -> Result<Arc<GpuImageFrame>, MediaError> {
            Ok(Arc::new(test_frame()))
        }
    }

    fn video_composition() -> Composition {
        let media_id = NodeId::new(1);
        let output_id = NodeId::new(2);
        let mut graph = Graph::new();
        graph.nodes.insert(
            media_id,
            NodeKind::MediaIn(MediaIn {
                id: media_id,
                kind: NodeProperty::Int(1),
                source: NodeProperty::String("video".to_string()),
                ..MediaIn::default()
            }),
        );
        graph.nodes.insert(
            output_id,
            NodeKind::MediaOutput(MediaOutput {
                id: output_id,
                source: PortRef::new(media_id, "output".to_string()),
            }),
        );
        graph
            .connect(Connection {
                from_node: media_id,
                from_port: "output".to_string(),
                to_node: output_id,
                to_port: "source".to_string(),
            })
            .expect("connect media output");

        Composition::new(
            graph,
            TimelineSettings {
                fps: 30.0,
                duration_frames: 120,
            },
            RenderSettings {
                width: 1,
                height: 1,
                background_color: [0, 0, 0, 255],
            },
        )
    }

    fn test_frame() -> GpuImageFrame {
        GpuImageFrame::from_cpu_decoded_rgba(
            &[0, 0, 0, 255],
            1,
            1,
            4,
            AlphaMode::Premultiplied,
            RectI::from_size(1, 1),
            RectI::from_size(1, 1),
        )
        .expect("test frame")
    }
}
