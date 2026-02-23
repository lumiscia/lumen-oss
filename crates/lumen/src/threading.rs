//! Threaded render orchestration placeholders.

use std::ops::Range;

use crate::{
	composition::Composition,
	error::LumenError,
	render::RenderContext,
	sink::Sink,
};

#[derive(Debug, Clone)]
pub struct RenderWorkerPool {
	pub worker_count: usize,
}

impl RenderWorkerPool {
	pub fn new(worker_count: usize) -> Self {
		Self {
			worker_count: worker_count.max(1),
		}
	}
}

#[derive(Debug, Clone)]
pub struct RenderOrchestrator {
	pub workers: RenderWorkerPool,
}

impl RenderOrchestrator {
	pub fn new(worker_count: usize) -> Self {
		Self {
			workers: RenderWorkerPool::new(worker_count),
		}
	}
}

impl Composition {
	pub fn render_sequence(
		&self,
		frame_range: Range<u32>,
		mut context: RenderContext,
		mut sink: Box<dyn Sink>,
		worker_count: usize,
	) -> Result<(), LumenError> {
		let _orchestrator = RenderOrchestrator::new(worker_count);
		for frame in frame_range {
			let bitmap = self.render_frame(frame, &mut context)?;
			sink.write_frame(frame, &bitmap)?;
		}
		sink.finalize()?;
		Ok(())
	}
}
