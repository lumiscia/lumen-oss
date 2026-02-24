#[cfg(feature = "threading")]
mod tests {
    use std::sync::{Arc, Mutex, RwLock};

    use lumen::error::SinkError;
    use lumen::{
        AssetCache, Composition, Connection, Graph, InputPort, LumenError, NodeId, NodeKind,
        NullMediaStore, OutputPort, RasterFrame, RenderContext, RenderSettings,
        RuntimeCapabilityProfile, Sink, SurfacePool, TimelineSettings,
        node::{Node, media_output::MediaOutput, solid_color::SolidColor},
    };

    #[derive(Default)]
    struct SinkState {
        frames: Vec<(u32, Arc<Vec<u8>>)>,
        finalized: bool,
    }

    struct SharedSink {
        state: Arc<Mutex<SinkState>>,
        cancel_after_frame: Option<u32>,
        cancellation: Option<lumen::CancellationToken>,
    }

    impl Sink for SharedSink {
        fn write_frame(&mut self, frame: u32, data: &RasterFrame) -> Result<(), SinkError> {
            if let Some(cancel_at) = self.cancel_after_frame {
                if frame >= cancel_at {
                    if let Some(token) = &self.cancellation {
                        token.cancel();
                    }
                }
            }

            let RasterFrame::Bitmap(bitmap) =
                data.clone()
                    .to_bitmap()
                    .map_err(|error| SinkError::WriteFrame {
                        frame,
                        details: error.to_string(),
                    })?
            else {
                return Err(SinkError::WriteFrame {
                    frame,
                    details: "expected bitmap frame".to_string(),
                });
            };

            if let Ok(mut state) = self.state.lock() {
                state.frames.push((frame, bitmap.pixels));
            }
            Ok(())
        }

        fn finalize(&mut self) -> Result<(), SinkError> {
            if let Ok(mut state) = self.state.lock() {
                state.finalized = true;
            }
            Ok(())
        }
    }

    fn render_context(composition: &Composition) -> RenderContext {
        RenderContext::new(
            composition,
            Arc::new(SurfacePool::new()),
            Arc::new(RwLock::new(AssetCache::new())),
            Arc::new(NullMediaStore),
            RuntimeCapabilityProfile::cpu_only(),
        )
    }

    fn baseline_composition(duration_frames: u32) -> Composition {
        let mut graph = Graph::new();
        let solid = graph.add_node(Node::new(
            NodeId(0),
            NodeKind::SolidColor(SolidColor {
                color: [25, 50, 75, 255],
                width: Some(2),
                height: Some(2),
            }),
        ));
        let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
        graph
            .connect(Connection {
                from_node: solid,
                from_port: OutputPort::default(),
                to_node: output,
                to_port: InputPort::named("source"),
            })
            .expect("valid solid->output connection");

        Composition::new(
            graph,
            TimelineSettings {
                fps: 30.0,
                duration_frames,
            },
            RenderSettings {
                width: 2,
                height: 2,
                background_color: [0, 0, 0, 0],
            },
        )
    }

    #[test]
    fn render_sequence_orders_frames_and_matches_single_thread_pixels() {
        let composition = baseline_composition(60);
        let mut single_context = render_context(&composition);
        let mut single_thread_frames = Vec::new();
        for frame in 0..60 {
            let rendered = composition
                .render_frame(frame, &mut single_context)
                .expect("single-thread render should succeed");
            let RasterFrame::Bitmap(bitmap) = rendered else {
                panic!("expected bitmap frame");
            };
            single_thread_frames.push((frame, bitmap.pixels));
            single_thread_frames.push((frame, bytes));
        }

        let state = Arc::new(Mutex::new(SinkState::default()));
        let sink = SharedSink {
            state: Arc::clone(&state),
            cancel_after_frame: None,
            cancellation: None,
        };
        composition
            .render_sequence(0..60, render_context(&composition), Box::new(sink), 4)
            .expect("threaded render should succeed");

        let state = state.lock().expect("sink state lock");
        assert!(state.finalized);
        assert_eq!(state.frames.len(), 60);
        for (index, (frame, pixels)) in state.frames.iter().enumerate() {
            assert_eq!(
                *frame, index as u32,
                "frames must be written in ascending order"
            );
            assert_eq!(pixels.as_ref(), single_thread_frames[index].1.as_ref());
        }
    }

    #[test]
    fn cancellation_stops_workers_and_still_finalizes_sink() {
        let composition = baseline_composition(60);
        let context = render_context(&composition);
        let cancellation = context.cancellation.clone();
        let state = Arc::new(Mutex::new(SinkState::default()));
        let sink = SharedSink {
            state: Arc::clone(&state),
            cancel_after_frame: Some(0),
            cancellation: Some(cancellation),
        };

        let result = composition.render_sequence(0..60, context, Box::new(sink), 4);
        assert!(matches!(
            result,
            Err(LumenError::Render(
                lumen::error::RenderError::Cancelled { .. }
            ))
        ));

        let state = state.lock().expect("sink state lock");
        assert!(state.finalized);
        assert!(!state.frames.is_empty());
    }

    #[test]
    fn worker_frame_error_propagates_with_context() {
        let composition = baseline_composition(10);
        let state = Arc::new(Mutex::new(SinkState::default()));
        let sink = SharedSink {
            state: Arc::clone(&state),
            cancel_after_frame: None,
            cancellation: None,
        };

        let result =
            composition.render_sequence(0..12, render_context(&composition), Box::new(sink), 4);
        assert!(matches!(
            result,
            Err(LumenError::Render(lumen::error::RenderError::FrameOutOfRange {
                frame,
                duration_frames: 10,
            })) if frame >= 10
        ));

        let state = state.lock().expect("sink state lock");
        assert!(state.finalized);
    }
}
