use std::collections::HashMap;

use axum::body::Bytes;
use lumen_ffmpeg::{EncoderQuality, parse_bitrate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use super::util::sanitize_error_message;

#[derive(Debug, Deserialize)]
pub struct RenderJobInput {
    pub composition: Value,
    #[serde(default)]
    pub media: HashMap<String, String>,
    #[serde(default)]
    #[serde(rename = "webhookUrl")]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub quality: Option<QualityInput>,
}

/// The encode quality a request asks for. At most one of `bitrate`, `crf` and `qp` may be set.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityInput {
    bitrate: Option<BitrateInput>,
    crf: Option<u8>,
    qp: Option<u8>,
    preset: Option<String>,
}

/// Bits per second as a number, or as text with the CLI's `k` and `M` suffixes, such as `"14M"`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BitrateInput {
    BitsPerSecond(u64),
    Text(String),
}

impl QualityInput {
    /// `None` when the request asked for neither a rate control nor a preset.
    pub fn into_quality(self) -> Result<Option<EncoderQuality>, RenderJobError> {
        let invalid = |message: String| RenderJobError {
            code: "invalid_quality".to_string(),
            message: sanitize_error_message(&message),
            retryable: false,
        };
        let bitrate = self
            .bitrate
            .map(|bitrate| match bitrate {
                BitrateInput::BitsPerSecond(bits) => parse_bitrate(&bits.to_string()),
                BitrateInput::Text(text) => parse_bitrate(&text),
            })
            .transpose()
            .map_err(invalid)?;
        EncoderQuality::from_options(bitrate, self.crf, self.qp, self.preset).map_err(invalid)
    }
}

/// Why a render failed, kept with the render and returned by every read of it.
#[derive(Debug, Clone, Serialize)]
pub struct RenderJobError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// A render and its latest status. Sockets subscribe to the status and polling reads it, so every
/// reader sees the same value.
pub struct RenderRecord {
    pub id: String,
    pub created_at: String,
    pub input_hash: String,
    pub status: watch::Sender<RenderStatus>,
}

#[derive(Debug, Clone)]
pub enum RenderStatus {
    Queued,
    Staging,
    Rendering {
        frame: u32,
        total: u32,
    },
    Succeeded {
        artifact: Artifact,
    },
    Failed {
        error: RenderJobError,
        progress: f32,
    },
}

#[derive(Debug, Clone)]
pub struct Artifact {
    /// Where the client downloads it from, relative to the server.
    pub url: String,
    pub bytes: Bytes,
    pub render_ms: u128,
}

impl RenderStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded { .. } | Self::Failed { .. })
    }

    /// `0` to `1`. Rendering fills `0.05` to `0.99`, and only success reaches `1`.
    pub fn progress(&self) -> f32 {
        match self {
            Self::Queued => 0.0,
            Self::Staging => 0.01,
            Self::Rendering { frame, total } => {
                let ratio = *frame as f32 / (*total).max(1) as f32;
                (0.05 + 0.90 * ratio).clamp(0.0, 0.99)
            }
            Self::Succeeded { .. } => 1.0,
            Self::Failed { progress, .. } => *progress,
        }
    }

    fn state(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Staging | Self::Rendering { .. } => "processing",
            Self::Succeeded { .. } => "succeeded",
            Self::Failed { .. } => "failed",
        }
    }

    fn stage(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Staging => "accepted",
            Self::Rendering { .. } => "rendering",
            Self::Succeeded { .. } => "completed",
            Self::Failed { .. } => "failed",
        }
    }

    fn error(&self) -> Option<&RenderJobError> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    fn artifact(&self) -> Option<&Artifact> {
        match self {
            Self::Succeeded { artifact } => Some(artifact),
            _ => None,
        }
    }
}

impl RenderRecord {
    pub fn new(id: String, created_at: String, input_hash: String) -> Self {
        Self {
            id,
            created_at,
            input_hash,
            status: watch::Sender::new(RenderStatus::Queued),
        }
    }

    /// Records a rendered frame. Subscribers are woken when the stage changes or the progress has
    /// grown by `min_delta` since the last time they were; the stored status is always current.
    pub fn set_frame(&self, frame: u32, total: u32, min_delta: f32, woken_at: &mut f32) {
        let next = RenderStatus::Rendering { frame, total };
        let progress = next.progress();
        self.status.send_if_modified(|status| {
            let wake = !matches!(status, RenderStatus::Rendering { .. })
                || progress - *woken_at >= min_delta;
            *status = next;
            if wake {
                *woken_at = progress;
            }
            wake
        });
    }
}

#[derive(Debug, Serialize)]
pub struct ApiRenderJob {
    #[serde(rename = "costMicros")]
    cost_micros: u64,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RenderJobError>,
    id: String,
    #[serde(rename = "inputHash")]
    input_hash: String,
    #[serde(rename = "organizationId")]
    organization_id: &'static str,
    #[serde(rename = "outputUrl", skip_serializing_if = "Option::is_none")]
    output_url: Option<String>,
    status: &'static str,
}

impl ApiRenderJob {
    pub fn new(record: &RenderRecord, status: &RenderStatus) -> Self {
        Self {
            cost_micros: 0,
            created_at: record.created_at.clone(),
            error: status.error().cloned(),
            id: record.id.clone(),
            input_hash: record.input_hash.clone(),
            organization_id: "self-hosted",
            output_url: status.artifact().map(|artifact| artifact.url.clone()),
            status: status.state(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateRenderResponse {
    pub cached: bool,
    pub render: ApiRenderJob,
}

#[derive(Debug, Serialize)]
pub struct GetRenderResponse {
    pub render: ApiRenderJob,
}

#[derive(Debug, Serialize)]
pub struct RenderQueueState {
    #[serde(rename = "artifactUrl", skip_serializing_if = "Option::is_none")]
    artifact_url: Option<String>,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(rename = "errorCode", skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
    #[serde(rename = "organizationId")]
    organization_id: &'static str,
    #[serde(rename = "outputBytes", skip_serializing_if = "Option::is_none")]
    output_bytes: Option<usize>,
    progress: f32,
    #[serde(rename = "renderId")]
    render_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
    stage: &'static str,
    state: &'static str,
}

impl RenderQueueState {
    pub fn new(id: &str, status: &RenderStatus) -> Self {
        let artifact = status.artifact();
        let error = status.error();
        Self {
            artifact_url: artifact.map(|artifact| artifact.url.clone()),
            duration_ms: artifact.map(|artifact| artifact.render_ms),
            error: error.map(|error| error.message.clone()),
            error_code: error.map(|error| error.code.clone()),
            organization_id: "self-hosted",
            output_bytes: artifact.map(|artifact| artifact.bytes.len()),
            progress: status.progress(),
            render_id: id.to_string(),
            retryable: error.map(|error| error.retryable),
            stage: status.stage(),
            state: status.state(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RenderProgressResponse {
    pub progress: RenderQueueState,
    pub render: ApiRenderJob,
}

#[cfg(test)]
mod tests {
    use lumen_ffmpeg::{EncoderQuality, RateControl};
    use serde_json::json;

    use super::QualityInput;

    fn quality(value: serde_json::Value) -> Option<EncoderQuality> {
        serde_json::from_value::<QualityInput>(value)
            .unwrap()
            .into_quality()
            .unwrap()
    }

    #[test]
    fn quality_maps_to_the_encoder_settings_the_cli_flags_would() {
        assert_eq!(
            quality(json!({ "crf": 23, "preset": "fast" })),
            Some(EncoderQuality {
                rate_control: RateControl::Crf(23),
                preset: Some("fast".into()),
            })
        );
        for bitrate in [json!("14M"), json!(14_000_000)] {
            assert_eq!(
                quality(json!({ "bitrate": bitrate })).unwrap().rate_control,
                RateControl::Bitrate(14_000_000)
            );
        }
        assert_eq!(quality(json!({})), None);
    }

    #[test]
    fn conflicting_or_invalid_quality_is_rejected() {
        for value in [
            json!({ "crf": 23, "qp": 20 }),
            json!({ "bitrate": "8M", "crf": 23 }),
            json!({ "bitrate": 0 }),
            json!({ "bitrate": "fast" }),
        ] {
            let error = serde_json::from_value::<QualityInput>(value)
                .unwrap()
                .into_quality()
                .unwrap_err();
            assert_eq!(error.code, "invalid_quality");
        }
        let long = serde_json::from_value::<QualityInput>(json!({ "bitrate": "x".repeat(10_000) }))
            .unwrap()
            .into_quality()
            .unwrap_err();
        assert!(long.message.chars().count() <= 1024);
        assert!(serde_json::from_value::<QualityInput>(json!({ "crf": 300 })).is_err());
        assert!(serde_json::from_value::<QualityInput>(json!({ "speed": 1 })).is_err());
    }
}
