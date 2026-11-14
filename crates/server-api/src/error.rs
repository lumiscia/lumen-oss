use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// The body of every error response, whatever the route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    pub code: ErrorCode,
    /// What went wrong, for a person to read.
    pub error: String,
    /// Each error in the composition, when it was refused as invalid. Each is a `Diagnostic` of the
    /// runtime schema.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::diagnostics")
    )]
    pub diagnostics: Vec<Value>,
    /// The media source that failed, when one did: the runtime schema's `MediaFailure`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::media_failure")
    )]
    pub media: Option<Value>,
}

macro_rules! error_codes {
    ($(#[doc = $doc:literal] $variant:ident = $code:literal,)*) => {
        /// Why a request or a render failed. The listed codes are `lumen-server`'s; another host can
        /// send others, and a render the engine failed has the engine's diagnostic code, such as
        /// `media.not_found`. Both arrive as [`Other`](Self::Other).
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum ErrorCode {
            $(#[doc = $doc] $variant,)*
            /// A code not listed here, such as an engine diagnostic code.
            Other(String),
        }

        impl ErrorCode {
            /// Each listed code and the line that documents it.
            pub const LISTED: &'static [(&'static str, &'static str)] = &[$(($code, $doc)),*];

            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $code,)*
                    Self::Other(code) => code,
                }
            }
        }

        impl From<&str> for ErrorCode {
            fn from(code: &str) -> Self {
                match code {
                    $($code => Self::$variant,)*
                    other => Self::Other(other.to_string()),
                }
            }
        }
    };
}

error_codes! {
    /// `401`: the bearer token is missing or wrong.
    Unauthorized = "unauthorized",
    /// `400` or `422`: the body is not JSON, or not the JSON the route takes.
    InvalidRequestBody = "invalid_request_body",
    /// `413`: the JSON body is over the server's limit.
    RequestTooLarge = "request_too_large",
    /// `404`: no route has this path.
    RouteNotFound = "route_not_found",
    /// `405`: the route doesn't take this method.
    MethodNotAllowed = "method_not_allowed",
    /// Another request the router refused, such as a path segment that doesn't parse.
    InvalidRequest = "invalid_request",
    /// `404`: no render has this id, or it was evicted.
    RenderNotFound = "render_not_found",
    /// `503`: every render slot is busy and the queue is full.
    RenderQueueFull = "render_queue_full",
    /// `409`: the render has not succeeded yet, so it has no artifact.
    ArtifactNotReady = "artifact_not_ready",
    /// `400`: `webhookUrl` was set, and `lumen-server` delivers no webhooks.
    WebhooksUnsupported = "webhooks_unsupported",
    /// `500`: checking the composition failed inside the server.
    CheckFailed = "check_failed",
    /// The render's worker failed inside the server.
    RenderWorkerFailed = "render_worker_failed",
    /// `422`: a media manifest alias is not a plain file name.
    InvalidMediaAlias = "invalid_media_alias",
    /// `422`: the media manifest has too many entries.
    MediaManifestTooLarge = "media_manifest_too_large",
    /// `422`: the payload is not a composition, or names media this server doesn't have.
    InvalidProjectPayload = "invalid_project_payload",
    /// `422`: the quality preset, codec or bit depth resolves to no encoder here.
    InvalidQuality = "invalid_quality",
    /// An encoder that doesn't exist, or a frame rate it can't encode.
    InvalidRenderProfile = "invalid_render_profile",
    /// The server's configuration names something that isn't there, such as a font directory.
    InvalidConfig = "invalid_config",
    /// `422`: the output can't be rendered, such as an empty frame range or encoder settings on an image.
    InvalidOutput = "invalid_output",
    /// An image sequence has more frames, or adds up to more bytes, than the server allows.
    OutputTooLarge = "output_too_large",
    /// The GPU work around the engine failed, such as reading a frame back.
    RenderFailed = "render_failed",
    /// The encoder failed to open, encode or finish.
    EncodeFailed = "encode_failed",
    /// The output couldn't be written or read back.
    OutputIo = "output_io",
    /// The render was cancelled.
    RenderCancelled = "render_cancelled",
    /// A media URL could not be downloaded.
    MediaDownloadFailed = "media_download_failed",
    /// A media URL's host has no public address and isn't allowed.
    MediaDownloadForbiddenHost = "media_download_forbidden_host",
    /// A downloaded media file is over its size limit.
    MediaDownloadTooLarge = "media_download_too_large",
    /// Downloaded media could not be written to the render's staging directory.
    MediaStageFailed = "media_stage_failed",
    /// `404`: no uploaded media has this id, or it expired.
    MediaNotFound = "media_not_found",
    /// `409`: the media already has its content.
    MediaAlreadyUploaded = "media_already_uploaded",
    /// `409`: the media's content is already being checked.
    MediaUploadInProgress = "media_upload_in_progress",
    /// `413`: the media is over the server's size limit, or the bytes sent are more than `sizeBytes`.
    MediaTooLarge = "media_too_large",
    /// `503`: as many uploads as the server allows are waiting for their content.
    MediaUploadLimit = "media_upload_limit",
    /// `400`: the bytes received are not `sizeBytes` long.
    MediaSizeMismatch = "media_size_mismatch",
    /// `404`: no multipart upload has this id.
    UploadNotFound = "upload_not_found",
    /// `409`: another request is already sending this part. Send it again once that ends.
    UploadPartInProgress = "upload_part_in_progress",
    /// `400`: the listed parts are not the parts uploaded.
    InvalidUploadParts = "invalid_upload_parts",
    /// `409`: the media has no validated content.
    MediaNotValidated = "media_not_validated",
    /// `500`: the server failed to store the media.
    MediaStoreFailed = "media_store_failed",
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Ok(Self::from(code.as_str()))
    }
}

/// A string listing each of [`ErrorCode::LISTED`] in `x-lumen-codes`, with its doc line.
#[cfg(feature = "metadata")]
impl schemars::JsonSchema for ErrorCode {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ErrorCode".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let codes: Vec<Value> = Self::LISTED
            .iter()
            .map(|(code, doc)| serde_json::json!({ "code": code, "description": doc.trim() }))
            .collect();
        schemars::json_schema!({
            "description": "Why a request or a render failed. The listed codes are lumen-server's; another host can send others, and a render the engine failed has the engine's diagnostic code, such as `media.not_found`.",
            "type": "string",
            "x-lumen-codes": codes
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_unknown_ones_are_kept() {
        for (code, _) in ErrorCode::LISTED {
            let parsed = ErrorCode::from(*code);
            assert!(!matches!(parsed, ErrorCode::Other(_)), "{code}");
            assert_eq!(serde_json::to_value(&parsed).unwrap(), *code);
        }
        let engine: ErrorCode = serde_json::from_str("\"media.not_found\"").unwrap();
        assert_eq!(engine, ErrorCode::Other("media.not_found".to_string()));
        assert_eq!(engine.as_str(), "media.not_found");
    }
}
