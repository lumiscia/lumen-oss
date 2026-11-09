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
    /// Multipart uploads by id.
    uploads: HashMap<String, Upload>,
    /// Bytes this media's uploads hold on disk before it is checked: stored parts and bodies still
    /// being written, across every upload. Never more than its `size_bytes`.
    staged_bytes: u64,
}

impl Entry {
    fn new(media: Media, expires_at_ms: u64) -> Self {
        Self {
            media,
            expires_at_ms,
            leases: 0,
            deleted: false,
            finishing: false,
            uploads: HashMap::new(),
            staged_bytes: 0,
        }
    }

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

/// A multipart upload: the parts received, and whether they are being joined.
#[derive(Default)]
struct Upload {
    parts: BTreeMap<u32, Part>,
    completing: bool,
}

struct Part {
    etag: String,
    bytes: u64,
}

#[derive(Debug)]
pub enum StoreError {
    NotFound,
    NotPending,
    Busy,
    TooLarge(u64),
    /// The bytes sent would hold more than the media's `size_bytes` on disk.
    ExceedsSize(u64),
    SizeMismatch {
        expected: u64,
        received: u64,
    },
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
            // Only a checked media's bytes are kept. Anything else a pending one holds on disk is
            // from uploads that ended with the last run, and nothing counts it.
            let _ = fs::remove_dir_all(path.join(PARTS_DIR));
            remove_temp_files(&path);
            if stored.media.status == MediaStatus::Pending {
                let _ = fs::remove_file(path.join(CONTENT_FILE));
            }
            entries.insert(
                id.to_string(),
                Entry::new(stored.media, stored.expires_at_ms),
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
        let entry = Entry::new(media.clone(), expires_at_ms);
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

    /// Removes `id`'s record now, so it stays gone after a restart, and its files now or once the
    /// renders holding it finish.
    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        let now = now_ms();
        let mut entries = self.entries();
        let entry = entries
            .get_mut(id)
            .filter(|entry| entry.visible(now))
            .ok_or(StoreError::NotFound)?;
        fs::remove_file(self.entry_dir(id).join(RECORD_FILE))?;
        if entry.leases > 0 {
            entry.deleted = true;
            return Ok(());
        }
        entries.remove(id);
        drop(entries);
        if let Err(error) = fs::remove_dir_all(self.entry_dir(id)) {
            tracing::warn!(media_id = id, %error, "failed to delete media files");
        }
        Ok(())
    }

    /// Counts `bytes` more toward what `id`'s uploads hold on disk, refusing to pass its size.
    fn stage(&self, id: &str, bytes: u64) -> Result<(), StoreError> {
        let mut entries = self.entries();
        let entry = entries.get_mut(id).ok_or(StoreError::NotFound)?;
        let staged = entry.staged_bytes.saturating_add(bytes);
        if staged > entry.media.size_bytes {
            return Err(StoreError::ExceedsSize(entry.media.size_bytes));
        }
        entry.staged_bytes = staged;
        Ok(())
    }

    fn unstage(&self, id: &str, bytes: u64) {
        if let Some(entry) = self.entries().get_mut(id) {
            entry.staged_bytes = entry.staged_bytes.saturating_sub(bytes);
        }
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
        let mut staging = Staging::new(self, id, &self.entry_dir(id));
        let received = staging.write(body).await?;
        if received != size {
            return Err(StoreError::SizeMismatch {
                expected: size,
                received,
            });
        }
        self.finish(id, staging.temp.clone()).await
    }

    pub fn start_multipart(&self, id: &str) -> Result<String, StoreError> {
        self.pending_size(id)?;
        let upload_id = format!("u_{}", uuid::Uuid::new_v4().simple());
        fs::create_dir_all(self.entry_dir(id).join(PARTS_DIR).join(&upload_id))?;
        let mut entries = self.entries();
        let entry = entries.get_mut(id).ok_or(StoreError::NotFound)?;
        entry.uploads.insert(upload_id.clone(), Upload::default());
        Ok(upload_id)
    }

    fn upload_dir(&self, id: &str, upload_id: &str) -> PathBuf {
        self.entry_dir(id).join(PARTS_DIR).join(upload_id)
    }

    /// Whether `upload_id` is an upload of pending `id` that is taking parts.
    fn check_upload(&self, id: &str, upload_id: &str) -> Result<(), StoreError> {
        self.pending_size(id)?;
        match self
            .entries()
            .get(id)
            .and_then(|entry| entry.uploads.get(upload_id))
        {
            Some(upload) if upload.completing => Err(StoreError::Busy),
            Some(_) => Ok(()),
            None => Err(StoreError::UploadNotFound),
        }
    }

    /// Stores part `number` of a multipart upload, replacing an earlier one, and returns its etag.
    /// A part being replaced counts until its replacement is stored.
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
        self.check_upload(id, upload_id)?;
        let dir = self.upload_dir(id, upload_id);
        let mut staging = Staging::new(self, id, &dir);
        let bytes = staging.write(body).await?;

        let etag = format!("\"{}\"", uuid::Uuid::new_v4().simple());
        let mut entries = self.entries();
        let entry = entries.get_mut(id).ok_or(StoreError::UploadNotFound)?;
        let upload = entry
            .uploads
            .get_mut(upload_id)
            .ok_or(StoreError::UploadNotFound)?;
        if upload.completing {
            return Err(StoreError::Busy);
        }
        // Renamed under the lock, so the file and its recorded size and etag always agree.
        fs::rename(&staging.temp, dir.join(number.to_string()))?;
        staging.kept = true;
        let replaced = upload.parts.insert(
            number,
            Part {
                etag: etag.clone(),
                bytes,
            },
        );
        if let Some(replaced) = replaced {
            entry.staged_bytes = entry.staged_bytes.saturating_sub(replaced.bytes);
        }
        Ok(etag)
    }

    /// Joins the listed parts, in order, into the file and checks it.
    pub async fn complete_multipart(
        &self,
        id: &str,
        upload_id: &str,
        parts: &[(u32, String)],
    ) -> Result<Media, StoreError> {
        let size = self.pending_size(id)?;
        if parts.is_empty() {
            return Err(StoreError::InvalidParts("no parts were listed".to_string()));
        }
        if parts.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(StoreError::InvalidParts(
                "parts must be listed in ascending part number order".to_string(),
            ));
        }
        {
            let mut entries = self.entries();
            let upload = entries
                .get_mut(id)
                .and_then(|entry| entry.uploads.get_mut(upload_id))
                .ok_or(StoreError::UploadNotFound)?;
            if upload.completing {
                return Err(StoreError::Busy);
            }
            let mut received = 0_u64;
            for (number, etag) in parts {
                match upload.parts.get(number) {
                    Some(part) if &part.etag == etag => received += part.bytes,
                    _ => {
                        return Err(StoreError::InvalidParts(format!(
                            "part {number} was not uploaded with that etag"
                        )));
                    }
                }
            }
            if received != size {
                return Err(StoreError::SizeMismatch {
                    expected: size,
                    received,
                });
            }
            upload.completing = true;
        }

        let joining = Joining {
            store: self.clone(),
            id: id.to_string(),
            upload_id: upload_id.to_string(),
            temp: temp_path(&self.entry_dir(id)),
        };
        // Created here, so the guard removes it even when the join outlives a dropped request.
        let output = fs::File::create(&joining.temp)?;
        let dir = self.upload_dir(id, upload_id);
        let files: Vec<PathBuf> = parts
            .iter()
            .map(|(number, _)| dir.join(number.to_string()))
            .collect();
        let received = tokio::task::spawn_blocking(move || join_files(&files, output, size))
            .await
            .map_err(|error| StoreError::Io(io::Error::other(error)))??;
        if received != size {
            return Err(StoreError::SizeMismatch {
                expected: size,
                received,
            });
        }
        self.finish(id, joining.temp.clone()).await
    }

    /// Moves complete bytes at `temp` into place and checks them. The media's uploads end, and
    /// their parts are deleted, once it is checked.
    async fn finish(&self, id: &str, temp: PathBuf) -> Result<Media, StoreError> {
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
        let mut finishing = Finishing {
            store: self.clone(),
            id: id.to_string(),
            content: self.entry_dir(id).join(CONTENT_FILE),
            done: false,
        };
        // Not awaited, so a cancel can't leave it running after the guard has cleaned up.
        fs::rename(&temp, &finishing.content)?;
        let probed = probe_blocking(finishing.content.clone()).await;

        // Recorded and then published under the lock a delete takes, so nothing can lease the
        // checked media or delete its record before the record is written, and a media deleted
        // first is never recorded again.
        let mut entries = self.entries();
        let entry = entries
            .get_mut(id)
            .filter(|entry| !entry.deleted)
            .ok_or(StoreError::NotFound)?;
        let mut stored = entry.stored();
        apply_probe(&mut stored.media, probed);
        self.persist(&stored)?;
        entry.media = stored.media.clone();
        entry.uploads.clear();
        entry.staged_bytes = 0;
        entry.finishing = false;
        finishing.done = true;
        drop(entries);
        let _ = fs::remove_dir_all(self.entry_dir(id).join(PARTS_DIR));
        Ok(stored.media)
    }

    /// The file downloaded to `target.file`, checked and kept as media from `source_url`.
    pub async fn import(
        &self,
        mut target: ImportDir,
        source_url: String,
        upload: NewUpload,
    ) -> Result<Media, StoreError> {
        let (mut media, expires_at_ms) = self.new_media(SourceType::Url, Some(source_url), upload);
        media.id = target.id.clone();
        let content = target.dir.join(CONTENT_FILE);
        // Not awaited, so a cancel can't leave it running while the guard removes the directory.
        fs::rename(&target.file, &content)?;
        let probed = probe_blocking(content).await;
        apply_probe(&mut media, probed);
        let media = self.insert(media, expires_at_ms)?;
        target.registered = true;
        Ok(media)
    }

    /// A directory for a URL import under a fresh id, with a temporary file in it to download to.
    pub fn import_dir(&self) -> io::Result<ImportDir> {
        let id = format!("m_{}", uuid::Uuid::new_v4().simple());
        let dir = self.entry_dir(&id);
        fs::create_dir_all(&dir)?;
        Ok(ImportDir {
            file: temp_path(&dir),
            id,
            dir,
            registered: false,
        })
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

/// A body being written for `id` to a temporary file, its bytes counted toward what `id`'s uploads
/// hold on disk as they arrive. Dropping it, even with its request, removes the file and stops
/// counting them, unless `kept` says a stored part now owns both.
struct Staging {
    store: AssetStore,
    id: String,
    temp: PathBuf,
    bytes: u64,
    kept: bool,
}

impl Staging {
    fn new(store: &AssetStore, id: &str, dir: &Path) -> Self {
        Self {
            store: store.clone(),
            id: id.to_string(),
            temp: temp_path(dir),
            bytes: 0,
            kept: false,
        }
    }

    /// Writes `body`, refusing a chunk that would take `id` past its size, and returns its length.
    async fn write<S, E>(&mut self, mut body: S) -> Result<u64, StoreError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: fmt::Display,
    {
        let mut file = tokio::fs::File::create(&self.temp).await?;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|error| StoreError::Body(error.to_string()))?;
            let length = chunk.len() as u64;
            self.store.stage(&self.id, length)?;
            self.bytes += length;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(self.bytes)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.kept {
            let _ = fs::remove_file(&self.temp);
            self.store.unstage(&self.id, self.bytes);
        }
    }
}

/// A media's complete bytes at `content`, being checked. Dropping it before `done`, even with its
/// request, removes them and lets the media take its upload again.
struct Finishing {
    store: AssetStore,
    id: String,
    content: PathBuf,
    done: bool,
}

impl Drop for Finishing {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let _ = fs::remove_file(&self.content);
        if let Some(entry) = self.store.entries().get_mut(&self.id) {
            entry.finishing = false;
        }
    }
}

/// A multipart upload's parts being joined into `temp`. Dropping it removes `temp` and lets the
/// upload take parts again, which a checked media no longer has.
struct Joining {
    store: AssetStore,
    id: String,
    upload_id: String,
    temp: PathBuf,
}

impl Drop for Joining {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temp);
        if let Some(upload) = self
            .store
            .entries()
            .get_mut(&self.id)
            .and_then(|entry| entry.uploads.get_mut(&self.upload_id))
        {
            upload.completing = false;
        }
    }
}

/// A URL import's directory under a fresh id, with `file` to download to. Dropping it, even with
/// its request, removes the directory unless [`AssetStore::import`] registered it.
pub struct ImportDir {
    id: String,
    dir: PathBuf,
    pub file: PathBuf,
    registered: bool,
}

impl Drop for ImportDir {
    fn drop(&mut self) {
        if !self.registered {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// Copies `files` in order into `output`, refusing more than `limit` bytes in all.
fn join_files(files: &[PathBuf], output: fs::File, limit: u64) -> io::Result<u64> {
    let mut output = io::BufWriter::new(output);
    let mut total = 0;
    for file in files {
        let mut part = io::Read::take(fs::File::open(file)?, limit - total + 1);
        total += io::copy(&mut part, &mut output)?;
        if total > limit {
            return Err(io::Error::other(format!(
                "the parts hold more than {limit} bytes"
            )));
        }
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

#[cfg(test)]
mod tests {
    use std::{future::Future, task::Poll};

    use super::*;

    #[tokio::test]
    async fn an_upload_whose_check_is_cancelled_can_be_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = AssetStore::open(dir.path(), Duration::from_secs(60), 1024).unwrap();
        let media = store
            .create(NewUpload {
                content_type: "image/png".to_string(),
                file_name: "fake.png".to_string(),
                size_bytes: 9,
            })
            .unwrap();
        let body = || stream_of(b"not media");

        // Dropped at its first await once the bytes are being checked, as a dropped request is.
        let mut upload = Box::pin(store.put_content(&media.id, body()));
        loop {
            match std::future::poll_fn(|cx| Poll::Ready(upload.as_mut().poll(cx))).await {
                Poll::Ready(result) => panic!("the upload finished uncancelled: {result:?}"),
                Poll::Pending if store.entries()[&media.id].finishing => break,
                Poll::Pending => tokio::task::yield_now().await,
            }
        }
        drop(upload);

        let retried = store.put_content(&media.id, body()).await.unwrap();
        assert_eq!(retried.status, MediaStatus::Rejected);
    }

    #[tokio::test]
    async fn a_check_cancelled_as_its_bytes_move_into_place_leaves_nothing_uncounted() {
        let dir = tempfile::tempdir().unwrap();
        let store = AssetStore::open(dir.path(), Duration::from_secs(60), 1024).unwrap();
        // A file operation left running by the cancel may land before or after its cleanup, so
        // each round gives it another chance to land after.
        for round in 0..20 {
            let id = store.create(new_upload(9)).unwrap().id;

            // Dropped at its first await once the bytes are being checked, as a dropped request is.
            let mut upload = Box::pin(store.put_content(&id, stream_of(b"not media")));
            loop {
                match std::future::poll_fn(|cx| Poll::Ready(upload.as_mut().poll(cx))).await {
                    Poll::Ready(result) => panic!("the upload finished uncancelled: {result:?}"),
                    Poll::Pending if store.entries()[&id].finishing => break,
                    Poll::Pending => tokio::task::yield_now().await,
                }
            }
            drop(upload);
            tokio::time::sleep(Duration::from_millis(50)).await;

            let upload_id = store.start_multipart(&id).unwrap();
            store
                .put_part(&id, &upload_id, 1, stream_of(b"not media"))
                .await
                .unwrap();
            let entry = dir.path().canonicalize().unwrap().join(&id);
            let held = bytes_under(&entry) - bytes_under(&entry.join(RECORD_FILE));
            assert!(
                held <= 9,
                "round {round}: {held} bytes held against a size of 9"
            );
        }
    }

    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xef, 0xa0, 0xf0, 0x1f, 0x00, 0x06, 0x00, 0x02, 0x5f, 0x34, 0xcd, 0x40, 0xc0, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn new_upload(size_bytes: usize) -> NewUpload {
        NewUpload {
            content_type: "image/png".to_string(),
            file_name: "pixel.png".to_string(),
            size_bytes: size_bytes as u64,
        }
    }

    fn stream_of(bytes: &'static [u8]) -> impl Stream<Item = Result<Bytes, io::Error>> + Unpin {
        futures_util::stream::iter([Ok(Bytes::from_static(bytes))])
    }

    #[tokio::test]
    async fn a_delete_during_completion_stays_deleted_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = AssetStore::open(dir.path(), Duration::from_secs(60), 1024).unwrap();
        let id = store.create(new_upload(TINY_PNG.len())).unwrap().id;
        let upload_id = store.start_multipart(&id).unwrap();
        let etag = store
            .put_part(&id, &upload_id, 1, stream_of(TINY_PNG))
            .await
            .unwrap();
        // Empty parts left out of the upload, which completion deletes, keep it busy for a while
        // after the bytes are checked.
        for number in 2..5_000 {
            store
                .put_part(&id, &upload_id, number, stream_of(b""))
                .await
                .unwrap();
        }

        // Leases and deletes the media the moment it reads as checked, as a render and a client
        // racing the completion would.
        let deleter = {
            let (store, id) = (store.clone(), id.clone());
            let started = std::time::Instant::now();
            std::thread::spawn(move || {
                loop {
                    assert!(
                        started.elapsed() < Duration::from_secs(60),
                        "the media never read as checked"
                    );
                    if store
                        .get(&id)
                        .is_ok_and(|media| media.status == MediaStatus::Validated)
                    {
                        let render = store.job_assets();
                        assert!(render.resolve(&id).is_some());
                        store.delete(&id).unwrap();
                        return render;
                    }
                    std::hint::spin_loop();
                }
            })
        };
        store
            .complete_multipart(&id, &upload_id, &[(1, etag)])
            .await
            .unwrap();
        let render = deleter.join().unwrap();

        let entry = dir.path().canonicalize().unwrap().join(&id);
        assert!(
            entry.join(CONTENT_FILE).exists(),
            "the render still reads it"
        );
        assert!(!entry.join(RECORD_FILE).exists(), "the delete was undone");
        drop(render);
        let reopened = AssetStore::open(dir.path(), Duration::from_secs(60), 1024).unwrap();
        assert!(matches!(reopened.get(&id), Err(StoreError::NotFound)));
    }

    #[tokio::test]
    async fn unchecked_bytes_a_crash_left_behind_do_not_count_against_a_new_upload() {
        let dir = tempfile::tempdir().unwrap();
        let id = AssetStore::open(dir.path(), Duration::from_secs(60), 1024)
            .unwrap()
            .create(new_upload(1024))
            .unwrap()
            .id;
        // What a crash leaves between moving the bytes into place and recording their check.
        let entry = dir.path().canonicalize().unwrap().join(&id);
        fs::write(entry.join(CONTENT_FILE), [0; 1024]).unwrap();

        let store = AssetStore::open(dir.path(), Duration::from_secs(60), 1024).unwrap();
        assert_eq!(store.get(&id).unwrap().status, MediaStatus::Pending);
        let upload_id = store.start_multipart(&id).unwrap();
        static PART: [u8; 1024] = [0; 1024];
        store
            .put_part(&id, &upload_id, 1, stream_of(&PART))
            .await
            .unwrap();
        let held: u64 = [CONTENT_FILE, PARTS_DIR]
            .iter()
            .map(|name| bytes_under(&entry.join(name)))
            .sum();
        assert_eq!(held, 1024, "only the new part is held");
    }

    fn bytes_under(path: &Path) -> u64 {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => fs::read_dir(path)
                .unwrap()
                .flatten()
                .map(|item| bytes_under(&item.path()))
                .sum(),
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        }
    }
}
