use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The body of `POST /media`, which creates media to upload the content of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CreateMediaRequest {
    /// 1 to 255 characters. The server decides what the file is from its bytes, not this.
    pub content_type: String,
    /// 1 to 255 characters.
    pub file_name: String,
    /// The exact size of the content to come.
    pub size_bytes: u64,
}

/// The body of `POST /media/url`, which downloads a file once and keeps it as media.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UrlMediaRequest {
    /// An `http` or `https` URL, fetched through the same host checks as a media manifest.
    pub url: String,
    /// Without one, the URL's last path segment, or `download`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

/// The body of `POST /media/{id}/multipart/{uploadId}/complete`: the parts to join, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CompleteMultipartRequest {
    pub parts: Vec<CompletedPart>,
}

/// One uploaded part, as uploading it answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CompletedPart {
    pub etag: String,
    pub part_number: u32,
}

/// The answer to `PUT /media/{id}/multipart/{uploadId}/parts/{partNumber}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PartResponse {
    pub etag: String,
    pub part_number: u32,
}

/// The answer to `POST /media/{id}/multipart`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MultipartUploadResponse {
    pub upload_id: String,
}

/// The answer to `POST /media`: the media, and where to `PUT` its content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UploadTargetResponse {
    pub media: Media,
    /// `lumen-server`'s is relative to the server: `/media/{id}/content`.
    pub upload_url: String,
}

/// The answer to reading media, uploading its content, completing a multipart upload and
/// `POST /media/url`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MediaResponse {
    pub media: Media,
}

/// Uploaded media. A composition uses it as `asset:<id>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Media {
    /// `m_` and 32 hex digits on `lumen-server`.
    pub id: String,
    pub content_type: String,
    /// RFC 3339 in UTC.
    pub created_at: String,
    /// When the media is deleted, as RFC 3339 in UTC.
    pub expires_at: String,
    pub file_name: String,
    /// Where the host keeps the file. `lumen-server` has none.
    pub object_key: Option<String>,
    /// Why the content was rejected, when `status` is `rejected`.
    pub rejection_reason: Option<String>,
    pub size_bytes: u64,
    pub source_type: MediaSource,
    /// The URL a `url` media was downloaded from.
    pub source_url: Option<String>,
    pub status: MediaStatus,
    /// The bytes received so far. `lumen-server` has none.
    pub uploaded_bytes: Option<u64>,
    /// What the content is, set once it was checked and readable.
    pub kind: Option<MediaKind>,
    /// What checking the content found, set with `kind`.
    pub metadata: Option<MediaMetadata>,
}

/// How media got its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum MediaSource {
    /// Sent to the server, whole or in parts.
    Upload,
    /// Downloaded by the server.
    Url,
}

/// Where media is. `lumen-server` sends these three; another host can send others, such as the
/// hosted API's `uploaded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "metadata", schemars(extend("x-lumen-open" = true)))]
#[serde(rename_all = "lowercase")]
pub enum MediaStatus {
    /// Waiting for its content.
    Pending,
    /// Its content was checked and can be rendered.
    Validated,
    /// Its content can't be used, as `rejectionReason` says.
    Rejected,
}

/// What the server decided media is by opening it, whatever its name and content type say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Font,
    Model,
    Environment,
}

/// What checking media found. Each kind fills its own fields: images their size; video its size,
/// frame rate, frames, duration, codec and audio; audio its duration, rate and channels; fonts their
/// families and faces; environment maps their size; models their animation clips, materials and
/// cameras.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MediaMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_audio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub families: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<Vec<FontFace>>,
    /// The clips a `model_3d`'s `animation` selects, each the runtime schema's `AnimationInfo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::animations")
    )]
    pub animations: Option<Vec<Value>>,
    /// The materials a `texture_material` selects, each the runtime schema's `MaterialInfo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::materials")
    )]
    pub materials: Option<Vec<Value>>,
    /// The cameras a `camera` selects, each the runtime schema's `CameraInfo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::cameras")
    )]
    pub cameras: Option<Vec<Value>>,
}

/// One face of an uploaded font.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct FontFace {
    pub family: String,
    pub weight: u16,
    /// `normal`, `italic` or `oblique`.
    pub style: String,
}
