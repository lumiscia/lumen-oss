use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::server::{
    assets::{Media, MediaKind, NewUpload, StoreError},
    refused_non_public_host,
};

use super::{errors::ApiError, state::AppState};

const MAX_FILE_NAME_CHARS: usize = 255;
const MAX_CONTENT_TYPE_CHARS: usize = 255;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateMediaInput {
    content_type: String,
    file_name: String,
    size_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UrlMediaInput {
    url: String,
    #[serde(default)]
    file_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CompleteInput {
    parts: Vec<CompletedPart>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletedPart {
    etag: String,
    part_number: u32,
}

#[derive(Serialize)]
pub(super) struct MediaResponse {
    media: Media,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadTargetResponse {
    media: Media,
    upload_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MultipartResponse {
    upload_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PartResponse {
    etag: String,
    part_number: u32,
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "media_not_found",
                "media was not found",
            ),
            StoreError::NotPending => Self::new(
                StatusCode::CONFLICT,
                "media_already_uploaded",
                "media already has its content",
            ),
            StoreError::Busy => Self::new(
                StatusCode::CONFLICT,
                "media_upload_in_progress",
                "media content is already being checked",
            ),
            StoreError::TooLarge(limit) => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "media_too_large",
                format!("media exceeds the {limit} byte limit"),
            ),
            StoreError::SizeMismatch { expected, received } => Self::new(
                StatusCode::BAD_REQUEST,
                "media_size_mismatch",
                format!("received {received} bytes, but sizeBytes is {expected}"),
            ),
            StoreError::UploadNotFound => Self::new(
                StatusCode::NOT_FOUND,
                "upload_not_found",
                "multipart upload was not found",
            ),
            StoreError::InvalidParts(message) => {
                Self::new(StatusCode::BAD_REQUEST, "invalid_upload_parts", message)
            }
            StoreError::NotValidated => Self::new(
                StatusCode::CONFLICT,
                "media_not_validated",
                "media has no validated content",
            ),
            StoreError::Body(message) => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_request_body",
                format!("failed reading the request body: {message}"),
            ),
            StoreError::Io(error) => {
                tracing::warn!(%error, "media store write failed");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "media_store_failed",
                    "failed to store media",
                )
            }
        }
    }
}

fn bounded(field: &str, value: String, max_chars: usize) -> Result<String, ApiError> {
    if value.is_empty() || value.chars().count() > max_chars {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_body",
            format!("{field} must be 1 to {max_chars} characters"),
        ));
    }
    Ok(value)
}

pub(super) async fn create_media(
    State(state): State<AppState>,
    input: Result<Json<CreateMediaInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(input) = input?;
    let media = state.assets.create(NewUpload {
        content_type: bounded("contentType", input.content_type, MAX_CONTENT_TYPE_CHARS)?,
        file_name: bounded("fileName", input.file_name, MAX_FILE_NAME_CHARS)?,
        size_bytes: input.size_bytes,
    })?;
    let upload_url = format!("/media/{}/content", media.id);
    Ok((
        StatusCode::CREATED,
        Json(UploadTargetResponse { media, upload_url }),
    )
        .into_response())
}

pub(super) async fn put_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Body,
) -> Result<Json<MediaResponse>, ApiError> {
    let media = state
        .assets
        .put_content(&id, body.into_data_stream())
        .await?;
    Ok(Json(MediaResponse { media }))
}

pub(super) async fn start_multipart(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let upload_id = state.assets.start_multipart(&id)?;
    Ok((StatusCode::CREATED, Json(MultipartResponse { upload_id })).into_response())
}

pub(super) async fn put_part(
    State(state): State<AppState>,
    Path((id, upload_id, part_number)): Path<(String, String, u32)>,
    body: Body,
) -> Result<Json<PartResponse>, ApiError> {
    let etag = state
        .assets
        .put_part(&id, &upload_id, part_number, body.into_data_stream())
        .await?;
    Ok(Json(PartResponse { etag, part_number }))
}

pub(super) async fn complete_multipart(
    State(state): State<AppState>,
    Path((id, upload_id)): Path<(String, String)>,
    input: Result<Json<CompleteInput>, JsonRejection>,
) -> Result<Json<MediaResponse>, ApiError> {
    let Json(input) = input?;
    let parts: Vec<(u32, String)> = input
        .parts
        .into_iter()
        .map(|part| (part.part_number, part.etag))
        .collect();
    let media = state
        .assets
        .complete_multipart(&id, &upload_id, &parts)
        .await?;
    Ok(Json(MediaResponse { media }))
}

/// Downloads `url` once, through the same address checks as manifest media, and keeps the file.
pub(super) async fn create_url_media(
    State(state): State<AppState>,
    input: Result<Json<UrlMediaInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(input) = input?;
    let file_name = match input.file_name {
        Some(name) => bounded("fileName", name, MAX_FILE_NAME_CHARS)?,
        None => file_name_of(&input.url),
    };
    let (dir, temp) = state.assets.import_dir().map_err(StoreError::from)?;
    let downloaded = state
        .media_client
        .download(
            &file_name,
            &input.url,
            &temp,
            state.assets.max_bytes(),
            None,
        )
        .await;
    let downloaded = match downloaded {
        Ok(downloaded) => downloaded,
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            let status = match error.code.as_str() {
                "media_download_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
                // A host this server won't fetch from isn't a transient failure.
                _ if refused_non_public_host(&error) => StatusCode::UNPROCESSABLE_ENTITY,
                _ if error.retryable => StatusCode::BAD_GATEWAY,
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            return Err(ApiError::rejected(status, error));
        }
    };
    let upload = NewUpload {
        content_type: downloaded
            .content_type
            .filter(|value| !value.is_empty() && value.len() <= MAX_CONTENT_TYPE_CHARS)
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        file_name,
        size_bytes: downloaded.bytes,
    };
    let imported = state.assets.import(&dir, temp, input.url, upload).await;
    let media = match imported {
        Ok(media) => media,
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Err(error.into());
        }
    };
    Ok((StatusCode::CREATED, Json(MediaResponse { media })).into_response())
}

/// The last path segment of `url`, or `download`.
fn file_name_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()?
                .next_back()
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
        })
        .filter(|name| name.chars().count() <= MAX_FILE_NAME_CHARS)
        .unwrap_or_else(|| "download".to_string())
}

pub(super) async fn get_media(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MediaResponse>, ApiError> {
    Ok(Json(MediaResponse {
        media: state.assets.get(&id)?,
    }))
}

pub(super) async fn delete_media(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.assets.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The validated bytes, served as the declared type only when it agrees with what they are.
pub(super) async fn get_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let (path, media) = state.assets.content(&id)?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| StoreError::NotFound)?;
    let length = file.metadata().await.map_err(StoreError::from)?.len();
    let body = stream::try_unfold(file, |mut file| async move {
        let mut buffer = vec![0; 64 * 1024];
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok::<_, std::io::Error>(None);
        }
        buffer.truncate(read);
        Ok(Some((Bytes::from(buffer), file)))
    });

    let content_type = media
        .kind
        .filter(|kind| declares_kind(&media.content_type, *kind))
        .and_then(|_| HeaderValue::from_str(&media.content_type).ok())
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type);
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok((StatusCode::OK, headers, Body::from_stream(body)).into_response())
}

fn declares_kind(content_type: &str, kind: MediaKind) -> bool {
    let content_type = content_type.to_ascii_lowercase();
    let prefixes: &[&str] = match kind {
        MediaKind::Image => &["image/"],
        MediaKind::Video => &["video/"],
        MediaKind::Audio => &["audio/"],
        MediaKind::Font => &["font/", "application/font-", "application/x-font-"],
    };
    prefixes
        .iter()
        .any(|prefix| content_type.starts_with(prefix))
        && !content_type.contains("svg")
}
