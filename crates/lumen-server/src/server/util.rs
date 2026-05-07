pub(super) fn sanitize_error_message(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.len() <= 256 {
        return trimmed.to_string();
    }
    trimmed.chars().take(256).collect()
}
