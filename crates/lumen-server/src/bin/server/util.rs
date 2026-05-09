use std::{
    hash::{Hash, Hasher},
    sync::atomic::{AtomicU64, Ordering},
};

static RENDER_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn new_render_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = RENDER_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("r_{nanos:x}{sequence:04x}")
}

pub(crate) fn current_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!("{seconds}")
}

pub(crate) fn input_hash(
    composition: &serde_json::Value,
    media: &std::collections::HashMap<String, String>,
) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    composition.to_string().hash(&mut hash);
    let mut media_entries = media.iter().collect::<Vec<_>>();
    media_entries.sort_by(|a, b| a.0.cmp(b.0));
    for (key, value) in media_entries {
        key.hash(&mut hash);
        value.hash(&mut hash);
    }
    format!("{:x}", hash.finish())
}

pub(super) fn sanitize_error_message(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.len() <= 256 {
        return trimmed.to_string();
    }
    trimmed.chars().take(256).collect()
}
