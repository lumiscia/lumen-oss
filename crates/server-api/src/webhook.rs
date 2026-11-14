use serde::{Deserialize, Serialize};

use crate::Render;

/// What a host that delivers webhooks posts to a render's `webhookUrl` when the render ends, as a
/// [Standard Webhooks](https://www.standardwebhooks.com) payload. The request is signed with the
/// `webhook-id`, `webhook-timestamp` and `webhook-signature` headers of that specification.
/// `lumen-server` delivers none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RenderWebhook {
    #[serde(rename = "type")]
    pub kind: RenderWebhookType,
    /// When the event happened, as RFC 3339 in UTC.
    pub timestamp: String,
    /// The render as it ended, as reading it returns it.
    pub data: Render,
}

/// Which end of a render a webhook reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
pub enum RenderWebhookType {
    /// The render succeeded, and `data.url` is where its output downloads from.
    #[serde(rename = "render.succeeded")]
    Succeeded,
    /// The render failed, and `data.error` says why.
    #[serde(rename = "render.failed")]
    Failed,
}

impl RenderWebhookType {
    pub const ALL: [Self; 2] = [Self::Succeeded, Self::Failed];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "render.succeeded",
            Self::Failed => "render.failed",
        }
    }
}
