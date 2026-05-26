use std::time::Duration;

use crate::gpu::{GpuBackend, GpuVideoInput};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuEncodeTelemetry {
    pub upload_attempts: u64,
    pub upload_successes: u64,
    pub upload_failures: u64,
    pub encode_attempts: u64,
    pub encode_successes: u64,
    pub encode_failures: u64,
    pub cuda_frames: u64,
    pub metal_frames: u64,
    pub vulkan_frames: u64,
    pub estimated_upload_bytes: u64,
    pub upload_time_us: u128,
    pub encode_time_us: u128,
    pub last_error: Option<String>,
    pub recent_events: Vec<GpuEncodeEvent>,
}

impl GpuEncodeTelemetry {
    const MAX_RECENT_EVENTS: usize = 128;

    pub fn record_upload_started(&mut self, descriptor: &GpuUploadDescriptor) {
        self.upload_attempts = self.upload_attempts.saturating_add(1);
        self.estimated_upload_bytes = self
            .estimated_upload_bytes
            .saturating_add(descriptor.estimated_bytes);
        match descriptor.backend {
            GpuBackend::Cuda => self.cuda_frames = self.cuda_frames.saturating_add(1),
            GpuBackend::Metal => self.metal_frames = self.metal_frames.saturating_add(1),
            GpuBackend::Vulkan => self.vulkan_frames = self.vulkan_frames.saturating_add(1),
        }
        self.push_event(GpuEncodeEvent::started(GpuEncodeStage::Upload, descriptor));
    }

    pub fn record_upload_finished(&mut self, descriptor: &GpuUploadDescriptor, elapsed: Duration) {
        self.upload_successes = self.upload_successes.saturating_add(1);
        self.upload_time_us = self.upload_time_us.saturating_add(elapsed.as_micros());
        self.push_event(GpuEncodeEvent::finished(
            GpuEncodeStage::Upload,
            descriptor,
            elapsed,
        ));
    }

    pub fn record_upload_failed(
        &mut self,
        descriptor: &GpuUploadDescriptor,
        elapsed: Duration,
        message: impl Into<String>,
    ) {
        let message = message.into();
        self.upload_failures = self.upload_failures.saturating_add(1);
        self.upload_time_us = self.upload_time_us.saturating_add(elapsed.as_micros());
        self.last_error = Some(message.clone());
        self.push_event(GpuEncodeEvent::failed(
            GpuEncodeStage::Upload,
            descriptor,
            elapsed,
            message,
        ));
    }

    pub fn record_encode_started(&mut self, descriptor: &GpuUploadDescriptor) {
        self.encode_attempts = self.encode_attempts.saturating_add(1);
        self.push_event(GpuEncodeEvent::started(GpuEncodeStage::Encode, descriptor));
    }

    pub fn record_encode_finished(&mut self, descriptor: &GpuUploadDescriptor, elapsed: Duration) {
        self.encode_successes = self.encode_successes.saturating_add(1);
        self.encode_time_us = self.encode_time_us.saturating_add(elapsed.as_micros());
        self.push_event(GpuEncodeEvent::finished(
            GpuEncodeStage::Encode,
            descriptor,
            elapsed,
        ));
    }

    pub fn record_encode_failed(
        &mut self,
        descriptor: &GpuUploadDescriptor,
        elapsed: Duration,
        message: impl Into<String>,
    ) {
        let message = message.into();
        self.encode_failures = self.encode_failures.saturating_add(1);
        self.encode_time_us = self.encode_time_us.saturating_add(elapsed.as_micros());
        self.last_error = Some(message.clone());
        self.push_event(GpuEncodeEvent::failed(
            GpuEncodeStage::Encode,
            descriptor,
            elapsed,
            message,
        ));
    }

    fn push_event(&mut self, event: GpuEncodeEvent) {
        if self.recent_events.len() == Self::MAX_RECENT_EVENTS {
            self.recent_events.remove(0);
        }
        self.recent_events.push(event);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuUploadDescriptor {
    pub backend: GpuBackend,
    pub width: u32,
    pub height: u32,
    pub estimated_bytes: u64,
}

impl GpuUploadDescriptor {
    pub fn from_frame(frame: &GpuVideoInput<'_>) -> Self {
        let (width, height) = frame.dimensions();
        Self {
            backend: frame.backend(),
            width,
            height,
            estimated_bytes: frame.estimated_rgba_bytes(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuEncodeStage {
    Upload,
    Encode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuEncodeEvent {
    pub stage: GpuEncodeStage,
    pub outcome: GpuEncodeOutcome,
    pub backend: GpuBackend,
    pub width: u32,
    pub height: u32,
    pub estimated_bytes: u64,
    pub elapsed_us: Option<u128>,
    pub message: Option<String>,
}

impl GpuEncodeEvent {
    fn started(stage: GpuEncodeStage, descriptor: &GpuUploadDescriptor) -> Self {
        Self::new(stage, GpuEncodeOutcome::Started, descriptor, None, None)
    }

    fn finished(
        stage: GpuEncodeStage,
        descriptor: &GpuUploadDescriptor,
        elapsed: Duration,
    ) -> Self {
        Self::new(
            stage,
            GpuEncodeOutcome::Finished,
            descriptor,
            Some(elapsed),
            None,
        )
    }

    fn failed(
        stage: GpuEncodeStage,
        descriptor: &GpuUploadDescriptor,
        elapsed: Duration,
        message: String,
    ) -> Self {
        Self::new(
            stage,
            GpuEncodeOutcome::Failed,
            descriptor,
            Some(elapsed),
            Some(message),
        )
    }

    fn new(
        stage: GpuEncodeStage,
        outcome: GpuEncodeOutcome,
        descriptor: &GpuUploadDescriptor,
        elapsed: Option<Duration>,
        message: Option<String>,
    ) -> Self {
        Self {
            stage,
            outcome,
            backend: descriptor.backend,
            width: descriptor.width,
            height: descriptor.height,
            estimated_bytes: descriptor.estimated_bytes,
            elapsed_us: elapsed.map(|duration| duration.as_micros()),
            message,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuEncodeOutcome {
    Started,
    Finished,
    Failed,
}
