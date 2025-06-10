use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use tracing::{error, warn};
use viz::{
    Body, Request, RequestExt, Response, StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};

use crate::{output::create_video, sequence::Sequence};

/*
async fn download_media(media: &Vec<Media>) -> Result<HashMap<usize, DownloadedMedia>, Response> {
    let (ok, err): (Vec<_>, Vec<_>) = futures_util::future::join_all(media.iter().map(|media| {
        let media_type = media.media_type.clone();
        let id = media.id.clone();
        let source = media.source.clone();

        download_medium(media_type, id, source)
    }))
    .await
    .into_iter()
    .partition(Result::is_ok);

    if !err.is_empty() {
        warn!("Failed to download source media");

        let errs: Vec<String> = err
            .into_iter()
            .map(|err| match err {
                Ok(_) => unreachable!(),
                Err(err) => format!("{:?}", err),
            })
            .collect();

        let body = json!({
            "error": "failed to download source media",
            "reasons": errs
        })
        .to_string();

        return Err(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .header(CONTENT_LENGTH, body.len())
            .body(Body::Full(body.into()))
            .unwrap());
    }

    Ok(ok
        .into_iter()
        .map(|res| {
            let medium = res.ok().unwrap();
            (medium.id, medium)
        })
        .collect())
}

async fn download_medium(
    media_type: MediaType,
    id: usize,
    source: String,
) -> anyhow::Result<DownloadedMedia> {
    let resp = reqwest::get(&source).await?;

    if resp.status() != StatusCode::OK {
        return Err(anyhow::anyhow!("Failed to download media from {}", source));
    }

    let bytes = resp.bytes().await?;

    Ok(DownloadedMedia::new(media_type, id, bytes.to_vec()))
}
*/

pub async fn generate(_: Request) -> viz::Result<Response> {
    /*
    let sequence: Sequence = match req.json().await {
        Ok(sequence) => sequence,
        Err(err) => return Err(err.into()),
    };
    */
    Ok(match create_video() {
        // create_video doesn't use request details in this example
        Ok(video_data) => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "video/mp4")
            .header(CONTENT_LENGTH, video_data.len().to_string())
            .body(Body::Full(video_data.into()))
            .unwrap(),
        Err(e) => {
            error!("Failed to create video: {:?}", e);
            let body = br#"{"error":"failed to create video"}"#.to_vec();

            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(CONTENT_TYPE, "application/json; charset=utf-8")
                .header(CONTENT_LENGTH, body.len().to_string())
                .body(Body::Full(body.into()))
                .unwrap()
        }
    })
}
