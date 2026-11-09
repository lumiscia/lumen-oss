//! Uploaded media: files checked once their bytes are complete, kept until they expire, and resolved
//! for `asset:` references.

mod probe;

use std::{
    collections::{BTreeMap, HashMap},
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use lumen_engine::media::{Asset, AssetResolver};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::util::rfc3339;

pub use probe::{MediaKind, MediaMetadata};

const RECORD_FILE: &str = "media.json";
const CONTENT_FILE: &str = "content";
const PARTS_DIR: &str = "parts";
const TEMP_PREFIX: &str = ".tmp-";
pub const MAX_PART_NUMBER: u32 = 10_000;

/// A media object as the API returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub id: String,
    pub content_type: String,
    pub created_at: String,
    pub expires_at: String,
    pub file_name: String,
    pub object_key: Option<String>,
    pub rejection_reason: Option<String>,
    pub size_bytes: u64,
    pub source_type: SourceType,
    pub source_url: Option<String>,
    pub status: MediaStatus,
    pub uploaded_bytes: Option<u64>,
    pub kind: Option<MediaKind>,
    pub metadata: Option<MediaMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    Upload,
    Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaStatus {
    Pending,
    Validated,
    Rejected,
}

/// What `media.json` holds.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    media: Media,
    expires_at_ms: u64,
}

struct Entry {
    media: Media,
    expires_at_ms: u64,
    /// Renders holding this asset, which keep it on disk past expiry or deletion.
    leases: usize,
    deleted: bool,
    /// Whether completed bytes are being checked, which no other write may race.
    finishing: bool,
    /// Multipart uploads by id, each with the etag of every part received.
    uploads: HashMap<String, BTreeMap<u32, String>>,
}

impl Entry {
    fn visible(&self, now_ms: u64) -> bool {
        !self.deleted && now_ms < self.expires_at_ms
    }

    fn stored(&self) -> Stored {
        Stored {
            media: self.media.clone(),
            expires_at_ms: self.expires_at_ms,
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    NotFound,
    NotPending,
    Busy,
    TooLarge(u64),
    SizeMismatch { expected: u64, received: u64 },
    UploadNotFound,
    InvalidParts(String),
    NotValidated,
    Body(String),
    Io(io::Error),
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The fields of a new upload, as `POST /media` gives them.
pub struct NewUpload {
    pub content_type: String,
    pub file_name: String,
    pub size_bytes: u64,
}

/// Uploaded media under one directory, one subdirectory per id.
#[derive(Clone)]
pub struct AssetStore {
    inner: Arc<Inner>,
}

struct Inner {
    dir: PathBuf,
    ttl: Duration,
    max_bytes: u64,
    entries: Mutex<HashMap<String, Entry>>,
}

impl fmt::Debug for AssetStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetStore")
            .field("dir", &self.inner.dir)
            .finish_non_exhaustive()
    }
}

impl AssetStore {
    /// Opens `dir`, creating it if needed, with the media a previous run left there. Multipart
    /// uploads in progress don't survive a restart.
    pub fn open(dir: &Path, ttl: Duration, max_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let dir = dir.canonicalize()?;
        let mut entries = HashMap::new();
        for item in fs::read_dir(&dir)? {
            let path = item?.path();
            let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_media_id(id) || !path.is_dir() {
                continue;
            }
            let stored = fs::read(path.join(RECORD_FILE))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
                .filter(|stored| stored.media.id == id);
            let Some(stored) = stored else {
                tracing::warn!(media_id = id, "removing media without a readable record");
                let _ = fs::remove_dir_all(&path);
                continue;
            };
            let _ = fs::remove_dir_all(path.join(PARTS_DIR));
            remove_temp_files(&path);
            entries.insert(
                id.to_string(),
                Entry {
                    media: stored.media,
                    expires_at_ms: stored.expires_at_ms,
                    leases: 0,
                    deleted: false,
                    finishing: false,
                    uploads: HashMap::new(),
                },
            );
        }
        tracing::info!(media_dir = %dir.display(), media = entries.len(), "opened media store");
        Ok(Self {
            inner: Arc::new(Inner {
                dir,
                ttl,
                max_bytes,
                entries: Mutex::new(entries),
            }),
        })
    }

    pub fn max_bytes(&self) -> u64 {
        self.inner.max_bytes
    }

    /// How often expired media is looked for: as often as it can expire, within a second to a minute.
    pub fn sweep_interval(&self) -> Duration {
        self.inner
            .ttl
            .clamp(Duration::from_secs(1), Duration::from_secs(60))
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn entry_dir(&self, id: &str) -> PathBuf {
        self.inner.dir.join(id)
    }

    fn new_media(
        &self,
        source_type: SourceType,
        source_url: Option<String>,
        upload: NewUpload,
    ) -> (Media, u64) {
        let now = now_ms();
        let expires_at_ms = now.saturating_add(self.inner.ttl.as_millis() as u64);
        let media = Media {
            id: format!("m_{}", uuid::Uuid::new_v4().simple()),
            content_type: upload.content_type,
            created_at: rfc3339(now / 1000),
            expires_at: rfc3339(expires_at_ms / 1000),
            file_name: upload.file_name,
            object_key: None,
            rejection_reason: None,
            size_bytes: upload.size_bytes,
            source_type,
            source_url,
            status: MediaStatus::Pending,
            uploaded_bytes: None,
            kind: None,
            metadata: None,
        };
        (media, expires_at_ms)
    }

    fn insert(&self, media: Media, expires_at_ms: u64) -> Result<Media, StoreError> {
        let entry = Entry {
            media: media.clone(),
            expires_at_ms,
            leases: 0,
            deleted: false,
            finishing: false,
            uploads: HashMap::new(),
        };
        self.persist(&entry.stored())?;
        self.entries().insert(media.id.clone(), entry);
        Ok(media)
    }

    /// Writes an entry's record, through a temporary file so a reader never sees half of one.
    fn persist(&self, stored: &Stored) -> io::Result<()> {
        let dir = self.entry_dir(&stored.media.id);
        fs::create_dir_all(&dir)?;
        let temp = temp_path(&dir);
        fs::write(&temp, serde_json::to_vec(stored)?)?;
        fs::rename(&temp, dir.join(RECORD_FILE))
    }

    /// A pending upload, waiting for its bytes.
    pub fn create(&self, upload: NewUpload) -> Result<Media, StoreError> {
        if upload.size_bytes > self.inner.max_bytes {
            return Err(StoreError::TooLarge(self.inner.max_bytes));
        }
        let (media, expires_at_ms) = self.new_media(SourceType::Upload, None, upload);
        self.insert(media, expires_at_ms)
    }

    pub fn get(&self, id: &str) -> Result<Media, StoreError> {
        let now = now_ms();
        self.entries()
            .get(id)
            .filter(|entry| entry.visible(now))
            .map(|entry| entry.media.clone())
            .ok_or(StoreError::NotFound)
    }

    /// The validated file behind `id`, with its declared content type and kind.
    pub fn content(&self, id: &str) -> Result<(PathBuf, Media), StoreError> {
        let media = self.get(id)?;
        if media.status != MediaStatus::Validated {
            return Err(StoreError::NotValidated);
        }
        Ok((self.entry_dir(id).join(CONTENT_FILE), media))
    }

    /// Removes `id` now, or once the renders holding it finish.
    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        let now = now_ms();
        let mut entries = self.entries();
        let entry = entries
            .get_mut(id)
            .filter(|entry| entry.visible(now))
            .ok_or(StoreError::NotFound)?;
        entry.deleted = true;
        if entry.leases == 0 {
            entries.remove(id);
            drop(entries);
            fs::remove_dir_all(self.entry_dir(id))?;
        }
        Ok(())
    }

    /// The declared size of a pending upload that isn't being checked.
    fn pending_size(&self, id: &str) -> Result<u64, StoreError> {
        let now = now_ms();
        let entries = self.entries();
        let entry = entries
            .get(id)
            .filter(|entry| entry.visible(now))
            .ok_or(StoreError::NotFound)?;
        if entry.media.status != MediaStatus::Pending {
            return Err(StoreError::NotPending);
        }
        if entry.finishing {
            return Err(StoreError::Busy);
        }
        Ok(entry.media.size_bytes)
    }

    /// Takes the whole file in one body, then checks it.
    pub async fn put_content<S, E>(&self, id: &str, body: S) -> Result<Media, StoreError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: fmt::Display,
    {
        let size = self.pending_size(id)?;
        let temp = temp_path(&self.entry_dir(id));
        let received = match write_body(&temp, body, size).await {
            Ok(received) => received,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
        };
        if received != size {
            let _ = fs::remove_file(&temp);
            return Err(StoreError::SizeMismatch {
                expected: size,
                received,
            });
        }
        self.finish(id, temp, |_| {}).await
    }

    pub fn start_multipart(&self, id: &str) -> Result<String, StoreError> {
        self.pending_size(id)?;
        let upload_id = format!("u_{}", uuid::Uuid::new_v4().simple());
        fs::create_dir_all(self.entry_dir(id).join(PARTS_DIR).join(&upload_id))?;
        let mut entries = self.entries();
        let entry = entries.get_mut(id).ok_or(StoreError::NotFound)?;
        entry.uploads.insert(upload_id.clone(), BTreeMap::new());
        Ok(upload_id)
    }

    fn upload_dir(&self, id: &str, upload_id: &str) -> PathBuf {
        self.entry_dir(id).join(PARTS_DIR).join(upload_id)
    }

    fn check_upload(&self, id: &str, upload_id: &str) -> Result<u64, StoreError> {
        let size = self.pending_size(id)?;
        let has_upload = self
            .entries()
            .get(id)
            .is_some_and(|entry| entry.uploads.contains_key(upload_id));
        if has_upload {
            Ok(size)
        } else {
            Err(StoreError::UploadNotFound)
        }
    }

    /// Stores part `number` of a multipart upload, replacing an earlier one, and returns its etag.
    pub async fn put_part<S, E>(
        &self,
        id: &str,
        upload_id: &str,
        number: u32,
        body: S,
    ) -> Result<String, StoreError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: fmt::Display,
    {
        if !(1..=MAX_PART_NUMBER).contains(&number) {
            return Err(StoreError::InvalidParts(format!(
                "part numbers run from 1 to {MAX_PART_NUMBER}"
            )));
        }
        let size = self.check_upload(id, upload_id)?;
        let dir = self.upload_dir(id, upload_id);
        let temp = temp_path(&dir);
        let written = async {
            write_body(&temp, body, size).await?;
            tokio::fs::rename(&temp, dir.join(number.to_string())).await?;
            Ok::<_, StoreError>(())
        }
        .await;
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        let etag = format!("\"{}\"", uuid::Uuid::new_v4().simple());
        let mut entries = self.entries();
        let parts = entries
            .get_mut(id)
            .and_then(|entry| entry.uploads.get_mut(upload_id))
            .ok_or(StoreError::UploadNotFound)?;
        parts.insert(number, etag.clone());
        Ok(etag)
    }

    /// Joins the listed parts, in order, into the file and checks it.
    pub async fn complete_multipart(
        &self,
        id: &str,
        upload_id: &str,
        parts: &[(u32, String)],
    ) -> Result<Media, StoreError> {
        let size = self.check_upload(id, upload_id)?;
        if parts.is_empty() {
            return Err(StoreError::InvalidParts("no parts were listed".to_string()));
        }
        if parts.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(StoreError::InvalidParts(
                "parts must be listed in ascending part number order".to_string(),
            ));
        }
        {
            let entries = self.entries();
            let received = entries
                .get(id)
                .and_then(|entry| entry.uploads.get(upload_id))
                .ok_or(StoreError::UploadNotFound)?;
            if let Some((number, _)) = parts
                .iter()
                .find(|(number, etag)| received.get(number) != Some(etag))
            {
                return Err(StoreError::InvalidParts(format!(
                    "part {number} was not uploaded with that etag"
                )));
            }
        }

        let dir = self.upload_dir(id, upload_id);
        let temp = temp_path(&self.entry_dir(id));
        let files: Vec<PathBuf> = parts
            .iter()
            .map(|(number, _)| dir.join(number.to_string()))
            .collect();
        let joined_temp = temp.clone();
        let received = tokio::task::spawn_blocking(move || join_files(&files, &joined_temp))
            .await
            .map_err(|error| StoreError::Io(io::Error::other(error)))?;
        let received = match received {
            Ok(received) => received,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error.into());
            }
        };
        if received != size {
            let _ = fs::remove_file(&temp);
            return Err(StoreError::SizeMismatch {
                expected: size,
                received,
            });
        }
        let media = self.finish(id, temp, |entry| entry.uploads.clear()).await?;
        let _ = fs::remove_dir_all(self.entry_dir(id).join(PARTS_DIR));
        Ok(media)
    }

    /// Moves complete bytes at `temp` into place and checks them, applying `on_done` to the entry.
    async fn finish(
        &self,
        id: &str,
        temp: PathBuf,
        on_done: impl FnOnce(&mut Entry),
    ) -> Result<Media, StoreError> {
        {
            let mut entries = self.entries();
            let entry = entries.get_mut(id).ok_or(StoreError::NotFound)?;
            if entry.media.status != MediaStatus::Pending {
                drop(entries);
                let _ = fs::remove_file(&temp);
                return Err(StoreError::NotPending);
            }
            if entry.finishing {
                drop(entries);
                let _ = fs::remove_file(&temp);
                return Err(StoreError::Busy);
            }
            entry.finishing = true;
        }
        let content = self.entry_dir(id).join(CONTENT_FILE);
        let probed = match tokio::fs::rename(&temp, &content).await {
            Ok(()) => Ok(probe_blocking(content).await),
            Err(error) => Err(error),
        };

        let mut entries = self.entries();
        let entry = entries.get_mut(id).ok_or(StoreError::NotFound)?;
        entry.finishing = false;
        let probed = match probed {
            Ok(probed) => probed,
            Err(error) => {
                drop(entries);
                let _ = fs::remove_file(&temp);
                return Err(error.into());
            }
        };
        apply_probe(&mut entry.media, probed);
        on_done(entry);
        let stored = entry.stored();
        drop(entries);
        self.persist(&stored)?;
        Ok(stored.media)
    }

    /// A file downloaded to `downloaded` in `dir`, which [`Self::import_dir`] made, checked and kept
    /// as media from `source_url`.
    pub async fn import(
        &self,
        dir: &Path,
        downloaded: PathBuf,
        source_url: String,
        upload: NewUpload,
    ) -> Result<Media, StoreError> {
        let id = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let (mut media, expires_at_ms) = self.new_media(SourceType::Url, Some(source_url), upload);
        media.id = id;
        let content = dir.join(CONTENT_FILE);
        tokio::fs::rename(&downloaded, &content).await?;
        let probed = probe_blocking(content).await;
        apply_probe(&mut media, probed);
        self.insert(media, expires_at_ms)
    }

    /// A directory for a URL import under a fresh id, with a temporary file in it to download to.
    pub fn import_dir(&self) -> io::Result<(PathBuf, PathBuf)> {
        let dir = self.entry_dir(&format!("m_{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir)?;
        let temp = temp_path(&dir);
        Ok((dir, temp))
    }

    /// Leases `id` for a render, so it stays on disk until [`Self::release`].
    fn lease(&self, id: &str) -> Option<Asset> {
        let now = now_ms();
        let mut entries = self.entries();
        let entry = entries.get_mut(id).filter(|entry| entry.visible(now))?;
        let asset = asset_of(&entry.media, &self.inner.dir)?;
        entry.leases += 1;
        Some(asset)
    }

    fn release(&self, id: &str) {
        if let Some(entry) = self.entries().get_mut(id) {
            entry.leases = entry.leases.saturating_sub(1);
        }
    }

    /// A resolver for one render, which leases every asset it resolves until it is dropped.
    pub fn job_assets(&self) -> JobAssets {
        JobAssets {
            store: self.clone(),
            leased: Mutex::default(),
        }
    }

    /// Deletes media that expired or was deleted, unless a render still holds it.
    pub fn sweep(&self) {
        let now = now_ms();
        let removed: Vec<String> = {
            let mut entries = self.entries();
            let removed: Vec<String> = entries
                .iter()
                .filter(|(_, entry)| entry.leases == 0 && !entry.visible(now))
                .map(|(id, _)| id.clone())
                .collect();
            for id in &removed {
                entries.remove(id);
            }
            removed
        };
        for id in removed {
            match fs::remove_dir_all(self.entry_dir(&id)) {
                Ok(()) => tracing::info!(media_id = id, "deleted expired media"),
                Err(error) => {
                    tracing::warn!(media_id = id, %error, "failed to delete expired media")
                }
            }
        }
    }
}

impl AssetResolver for AssetStore {
    fn resolve(&self, id: &str) -> Option<Asset> {
        let now = now_ms();
        let entries = self.entries();
        let entry = entries.get(id).filter(|entry| entry.visible(now))?;
        asset_of(&entry.media, &self.inner.dir)
    }
}

/// The assets one render resolved, which stay on disk until the render is done with them.
#[derive(Debug)]
pub struct JobAssets {
    store: AssetStore,
    leased: Mutex<HashMap<String, Asset>>,
}

impl AssetResolver for JobAssets {
    fn resolve(&self, id: &str) -> Option<Asset> {
        let mut leased = self.leased.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(asset) = leased.get(id) {
            return Some(asset.clone());
        }
        let asset = self.store.lease(id)?;
        leased.insert(id.to_string(), asset.clone());
        Some(asset)
    }
}

impl Drop for JobAssets {
    fn drop(&mut self) {
        let leased = self
            .leased
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for id in leased.keys() {
            self.store.release(id);
        }
    }
}

/// Only validated media resolves. Pending and rejected media count as missing.
fn asset_of(media: &Media, dir: &Path) -> Option<Asset> {
    if media.status != MediaStatus::Validated {
        return None;
    }
    let content = dir.join(&media.id).join(CONTENT_FILE);
    Some(match media.kind? {
        MediaKind::Image => Asset::Image(content),
        MediaKind::Video => Asset::Video(content),
        MediaKind::Audio => Asset::Audio(content),
        MediaKind::Font => Asset::Font(vec![content]),
    })
}

fn apply_probe(media: &mut Media, probed: Result<(MediaKind, MediaMetadata), String>) {
    media.uploaded_bytes = Some(media.size_bytes);
    match probed {
        Ok((kind, metadata)) => {
            media.status = MediaStatus::Validated;
            media.kind = Some(kind);
            media.metadata = Some(metadata);
        }
        Err(reason) => {
            media.status = MediaStatus::Rejected;
            media.rejection_reason = Some(reason);
        }
    }
}

async fn probe_blocking(path: PathBuf) -> Result<(MediaKind, MediaMetadata), String> {
    tokio::task::spawn_blocking(move || probe::probe(&path))
        .await
        .unwrap_or_else(|error| Err(format!("the file could not be checked: {error}")))
}

/// Writes `body` to `path`, refusing more than `limit` bytes, and returns how many it wrote.
async fn write_body<S, E>(path: &Path, mut body: S, limit: u64) -> Result<u64, StoreError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: fmt::Display,
{
    let mut file = tokio::fs::File::create(path).await?;
    let mut written = 0_u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| StoreError::Body(error.to_string()))?;
        written += chunk.len() as u64;
        if written > limit {
            return Err(StoreError::TooLarge(limit));
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(written)
}

fn join_files(files: &[PathBuf], into: &Path) -> io::Result<u64> {
    let mut output = io::BufWriter::new(fs::File::create(into)?);
    let mut total = 0;
    for file in files {
        total += io::copy(&mut fs::File::open(file)?, &mut output)?;
    }
    io::Write::flush(&mut output)?;
    Ok(total)
}

fn temp_path(dir: &Path) -> PathBuf {
    dir.join(format!("{TEMP_PREFIX}{}", uuid::Uuid::new_v4().simple()))
}

fn remove_temp_files(dir: &Path) {
    let Ok(items) = fs::read_dir(dir) else {
        return;
    };
    for item in items.flatten() {
        if item.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            let _ = fs::remove_file(item.path());
        }
    }
}

/// Ids are `m_` and 32 lowercase hex digits, so one can only ever name a directory of the store.
pub fn is_media_id(id: &str) -> bool {
    id.strip_prefix("m_").is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
