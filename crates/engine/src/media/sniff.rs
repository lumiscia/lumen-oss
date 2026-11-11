//! Telling what kind of media a file is from its first bytes.

use super::AssetKind;

/// The kind of media the start of a file, `head`, says it is, if it is one of the formats Lumen
/// knows. A file the engine can't read may still be recognised, so that a video given where an
/// image is wanted is reported as one.
pub fn sniff_kind(head: &[u8]) -> Option<AssetKind> {
    let starts = |magic: &[u8]| head.starts_with(magic);
    let riff = |form: &[u8]| starts(b"RIFF") && head.get(8..12) == Some(form);
    if starts(b"glTF") {
        return Some(AssetKind::Model);
    }
    if starts(b"#?RADIANCE") || starts(b"#?RGBE") || starts(&[0x76, 0x2f, 0x31, 0x01]) {
        return Some(AssetKind::Environment);
    }
    if starts(&[0x89, b'P', b'N', b'G'])
        || starts(&[0xff, 0xd8, 0xff])
        || starts(b"GIF8")
        || starts(b"II*\0")
        || starts(b"MM\0*")
        || riff(b"WEBP")
    {
        return Some(AssetKind::Image);
    }
    if starts(&[0x1a, 0x45, 0xdf, 0xa3]) || riff(b"AVI ") {
        return Some(AssetKind::Video);
    }
    if head.get(4..8) == Some(b"ftyp") {
        return match head.get(8..12) {
            Some(b"heic" | b"heix" | b"mif1" | b"avif") => Some(AssetKind::Image),
            Some(b"M4A " | b"M4B ") => Some(AssetKind::Audio),
            _ => Some(AssetKind::Video),
        };
    }
    if starts(b"ID3")
        || starts(b"fLaC")
        || starts(b"OggS")
        || riff(b"WAVE")
        || (starts(b"FORM") && head.get(8..12) == Some(b"AIFF"))
        || (head.len() >= 2 && head[0] == 0xff && head[1] & 0xe0 == 0xe0)
    {
        return Some(AssetKind::Audio);
    }
    if starts(&[0, 1, 0, 0]) || starts(b"OTTO") || starts(b"true") || starts(b"ttcf") {
        return Some(AssetKind::Font);
    }
    if starts(b"wOFF") || starts(b"wOF2") {
        return Some(AssetKind::Font);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_recognised_by_their_first_bytes() {
        let cases: [(&[u8], Option<AssetKind>); 12] = [
            (b"\x89PNG\r\n\x1a\n", Some(AssetKind::Image)),
            (b"RIFF\0\0\0\0WEBPVP8 ", Some(AssetKind::Image)),
            (b"\0\0\0\x18ftypmp42", Some(AssetKind::Video)),
            (b"\0\0\0\x18ftypM4A ", Some(AssetKind::Audio)),
            (b"\x1a\x45\xdf\xa3\x01", Some(AssetKind::Video)),
            (b"RIFF\0\0\0\0WAVEfmt ", Some(AssetKind::Audio)),
            (b"ID3\x04\0", Some(AssetKind::Audio)),
            (b"\0\x01\0\0\0\x10", Some(AssetKind::Font)),
            (b"wOFF\0\x01", Some(AssetKind::Font)),
            (b"glTF\x02\0\0\0", Some(AssetKind::Model)),
            (b"#?RADIANCE\n", Some(AssetKind::Environment)),
            (b"just some text", None),
        ];
        for (head, kind) in cases {
            assert_eq!(sniff_kind(head), kind, "{head:?}");
        }
    }
}
