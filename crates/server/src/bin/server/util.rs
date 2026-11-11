use std::hash::{Hash, Hasher};

pub(crate) fn new_render_id() -> String {
    format!("r_{}", uuid::Uuid::new_v4().simple())
}

/// The current time as RFC 3339 in UTC, to the second.
pub(crate) fn current_timestamp() -> String {
    rfc3339(unix_now())
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(crate) fn rfc3339(unix_seconds: u64) -> String {
    let (days, time) = (unix_seconds / 86_400, unix_seconds % 86_400);
    // Civil date from days since 1970-01-01, counting years from March so leap days come last.
    let shifted = days as i64 + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        time % 3_600 / 60,
        time % 60
    )
}

pub(crate) fn input_hash(
    composition: &serde_json::Value,
    media: &std::collections::HashMap<String, String>,
    encoder: Option<&lumen_ffmpeg::VideoEncoderChoice>,
    output: &lumen_server::render::RenderOutput,
) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    composition.to_string().hash(&mut hash);
    encoder.hash(&mut hash);
    if *output != lumen_server::render::RenderOutput::default() {
        serde_json::to_string(output)
            .unwrap_or_default()
            .hash(&mut hash);
    }
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
pub(super) const MAX_ERROR_MESSAGE_CHARS: usize = 1024;

/// `message` without the absolute file paths in it, each replaced by `<path>`: where the server
/// keeps its files is not for a client to learn. A path is a word that starts with `/` after
/// whitespace, a quote or a bracket and has more than one segment.
pub(super) fn hide_server_paths(message: &str) -> String {
    const OPENERS: &str = "`'\"([=";
    const ENDERS: &str = "`'\")]<>,;";
    let mut hidden = String::with_capacity(message.len());
    let mut previous: Option<char> = None;
    let mut chars = message.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let at_word_start = previous.is_none_or(|p| p.is_whitespace() || OPENERS.contains(p));
        if c == '/' && at_word_start {
            let mut end = start + 1;
            while let Some(&(at, next)) = chars.peek() {
                if next.is_whitespace() || ENDERS.contains(next) {
                    break;
                }
                end = at + next.len_utf8();
                chars.next();
            }
            let word = &message[start..end];
            let path = word.trim_end_matches([':', '.']);
            if path.matches('/').count() >= 2 {
                hidden.push_str("<path>");
                hidden.push_str(&word[path.len()..]);
            } else {
                hidden.push_str(word);
            }
            previous = word.chars().last();
        } else {
            hidden.push(c);
            previous = Some(c);
        }
    }
    hidden
}

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
    fn server_paths_are_hidden_and_other_text_is_kept() {
        assert_eq!(
            hide_server_paths(
                "audio `asset:a` failed: avformat_open_input: /var/lib/lumen/assets/ab12/file.wav: No such file (path `/srv/media/a b.wav`)"
            ),
            "audio `asset:a` failed: avformat_open_input: <path>: No such file (path `<path> b.wav`)"
        );
        let kept = "image `https://x.test/a/b.png` at /nodes/1 is 3/4 of a/b, see `/root`";
        assert_eq!(hide_server_paths(kept), kept.replace("/nodes/1", "<path>"));
    }

    #[test]
    fn timestamps_are_rfc3339_in_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_793_743_787), "2026-11-03T22:09:47Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(4_107_542_399), "2100-02-28T23:59:59Z");
    }

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
