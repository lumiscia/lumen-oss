use std::hash::{Hash, Hasher};

pub(crate) fn new_render_id() -> String {
    format!("r_{}", uuid::Uuid::new_v4().simple())
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

/// Long enough for an FFmpeg or expression error and the codec, stream, node or frame it ends
/// with, which are what a client needs to act on.
const MAX_ERROR_MESSAGE_CHARS: usize = 1024;

pub(super) fn sanitize_error_message(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.chars().count() <= MAX_ERROR_MESSAGE_CHARS {
        return trimmed.to_string();
    }
    trimmed.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_context_a_long_error_ends_with_and_bounds_the_rest() {
        let suffixed = format!("{} codec=H264 stream=0", "é".repeat(600));
        assert_eq!(sanitize_error_message(&suffixed), suffixed);

        let huge = "x".repeat(10 * MAX_ERROR_MESSAGE_CHARS);
        assert_eq!(
            sanitize_error_message(&huge).chars().count(),
            MAX_ERROR_MESSAGE_CHARS
        );
    }
}
