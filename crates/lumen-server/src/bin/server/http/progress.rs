use crate::server::{RenderNotification, RenderQueueState, current_timestamp};

use super::state::AppState;

#[derive(Default)]
pub(super) struct RenderProgressPatch {
    pub artifact_url: Option<String>,
    pub duration_ms: Option<u128>,
    pub error: Option<String>,
    pub output_bytes: Option<usize>,
    pub progress: Option<f32>,
    pub stage: Option<&'static str>,
    pub state: Option<&'static str>,
}

pub(super) fn update_render_progress(
    state: &AppState,
    id: &str,
    kind: &'static str,
    patch: RenderProgressPatch,
) {
    let notification = if let Ok(mut renders) = state.renders.write() {
        let Some(stored) = renders.get_mut(id) else {
            return;
        };
        let previous_broadcast = stored.last_progress_broadcast.clone();
        let mut progress = stored.progress.clone().unwrap_or_else(|| RenderQueueState {
            artifact_url: None,
            duration_ms: None,
            error: None,
            organization_id: "self-hosted".to_string(),
            output_bytes: None,
            progress: 0.0,
            render_id: id.to_string(),
            resolution: None,
            stage: Some("queued"),
            state: "queued",
            updated_at: current_timestamp(),
        });
        progress.artifact_url = patch.artifact_url.or(progress.artifact_url);
        progress.duration_ms = patch.duration_ms.or(progress.duration_ms);
        progress.error = patch.error.or(progress.error);
        progress.output_bytes = patch.output_bytes.or(progress.output_bytes);
        progress.progress = patch
            .progress
            .map(|value| value.clamp(progress.progress, 1.0))
            .unwrap_or(progress.progress);
        progress.stage = patch.stage.or(progress.stage);
        progress.state = patch.state.unwrap_or(progress.state);
        progress.updated_at = current_timestamp();
        stored.progress = Some(progress.clone());
        if should_broadcast_progress(kind, previous_broadcast.as_ref(), &progress, state) {
            stored.last_progress_broadcast = Some(progress.clone());
            Some(RenderNotification {
                state: Some(progress),
                kind,
            })
        } else {
            None
        }
    } else {
        None
    };
    if let Some(notification) = notification {
        broadcast_progress(state, notification.kind, notification.state);
    }
}

pub(super) fn broadcast_progress(
    state: &AppState,
    kind: &'static str,
    progress: Option<RenderQueueState>,
) {
    let _ = state.progress_tx.send(RenderNotification {
        state: progress,
        kind,
    });
}

fn should_broadcast_progress(
    kind: &'static str,
    previous: Option<&RenderQueueState>,
    next: &RenderQueueState,
    state: &AppState,
) -> bool {
    if kind != "progress" || matches!(next.state, "queued" | "succeeded" | "failed") {
        return true;
    }

    let Some(previous) = previous else {
        return true;
    };

    if previous.stage != next.stage || previous.state != next.state {
        return true;
    }

    next.progress - previous.progress >= state.progress_min_delta
}
