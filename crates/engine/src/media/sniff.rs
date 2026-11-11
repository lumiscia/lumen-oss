//! Telling what kind of media a file is from its first bytes.

use super::AssetKind;

/// What the start of a file says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sniff {
    /// A format that holds one kind of media only.
    Kind(AssetKind),
    /// A container of video and audio streams, such as MP4, Matroska/WebM or Ogg, which can hold
    /// either or both: only opening it says which.
    Streams,
    Unknown,
}

impl Sniff {
    /// For a file read as `wanted`: `Some(found)` if its start shows it is not that, with the kind
    /// it is when that is established, or `None` when it may well be.
    pub fn other_than(self, wanted: AssetKind) -> Option<Option<AssetKind>> {
        match self {
            Self::Kind(kind) if kind != wanted => Some(Some(kind)),
            Self::Streams if !matches!(wanted, AssetKind::Video | AssetKind::Audio) => Some(None),
            _ => None,
        }
    }
}

/// The kind of media the start of a file, `head`, says it is, if it is one of the formats Lumen
/// knows. A file the engine can't read may still be recognised, so that a video given where an
/// image is wanted is reported as one. A container that can hold audio or video is not given a
/// kind, as its header can't tell them apart.
pub fn sniff_kind(head: &[u8]) -> Option<AssetKind> {
    match sniff(head) {
        Sniff::Kind(kind) => Some(kind),
        Sniff::Streams | Sniff::Unknown => None,
    }
}

/// What the start of a file, `head`, says it is.
pub fn sniff(head: &[u8]) -> Sniff {
    use AssetKind::{Audio, Environment, Font, Image, Model};

    let starts = |magic: &[u8]| head.starts_with(magic);
    let riff = |form: &[u8]| starts(b"RIFF") && head.get(8..12) == Some(form);
    if starts(b"glTF") {
        return Sniff::Kind(Model);
    }
    if starts(b"#?RADIANCE") || starts(b"#?RGBE") || starts(&[0x76, 0x2f, 0x31, 0x01]) {
        return Sniff::Kind(Environment);
    }
    if starts(&[0x89, b'P', b'N', b'G'])
        || starts(&[0xff, 0xd8, 0xff])
        || starts(b"GIF8")
        || starts(b"II*\0")
        || starts(b"MM\0*")
        || riff(b"WEBP")
    {
        return Sniff::Kind(Image);
    }
    if head.get(4..8) == Some(b"ftyp") {
        return match head.get(8..12) {
            Some(b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"mif2" | b"avif") => {
                Sniff::Kind(Image)
            }
            Some(b"M4A " | b"M4B ") => Sniff::Kind(Audio),
            _ => Sniff::Streams,
        };
    }
    if starts(&[0x1a, 0x45, 0xdf, 0xa3])
        || starts(b"OggS")
        || starts(b"FLV\x01")
        || starts(&[0x30, 0x26, 0xb2, 0x75])
        || starts(&[0, 0, 1, 0xba])
        || riff(b"AVI ")
    {
        return Sniff::Streams;
    }
    // An MPEG audio frame header: 11 sync bits, then a version and layer that aren't reserved.
    let mpeg_audio = head.len() >= 2
        && head[0] == 0xff
        && head[1] & 0xe0 == 0xe0
        && head[1] >> 3 & 3 != 1
        && head[1] >> 1 & 3 != 0;
    if starts(b"ID3")
        || starts(b"fLaC")
        || riff(b"WAVE")
        || (starts(b"FORM") && head.get(8..12) == Some(b"AIFF"))
        || mpeg_audio
    {
        return Sniff::Kind(Audio);
    }
    if starts(&[0, 1, 0, 0])
        || starts(b"OTTO")
        || starts(b"true")
        || starts(b"ttcf")
        || starts(b"wOFF")
        || starts(b"wOF2")
    {
        return Sniff::Kind(Font);
    }
    Sniff::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_recognised_by_their_first_bytes() {
        let cases: [(&[u8], Sniff); 20] = [
            (b"\x89PNG\r\n\x1a\n", Sniff::Kind(AssetKind::Image)),
            (b"RIFF\0\0\0\0WEBPVP8 ", Sniff::Kind(AssetKind::Image)),
            (b"\0\0\0\x18ftypheic", Sniff::Kind(AssetKind::Image)),
            (b"\0\0\0\x18ftypavif", Sniff::Kind(AssetKind::Image)),
            (b"\0\0\0\x18ftypmif1", Sniff::Kind(AssetKind::Image)),
            (b"\0\0\0\x18ftypM4A ", Sniff::Kind(AssetKind::Audio)),
            (b"\0\0\0\x18ftypmp42", Sniff::Streams),
            (b"\0\0\0\x18ftypisom", Sniff::Streams),
            (b"\x1a\x45\xdf\xa3\x01", Sniff::Streams),
            (b"OggS\0\x02", Sniff::Streams),
            (b"RIFF\0\0\0\0AVI LIST", Sniff::Streams),
            (b"RIFF\0\0\0\0WAVEfmt ", Sniff::Kind(AssetKind::Audio)),
            (b"ID3\x04\0", Sniff::Kind(AssetKind::Audio)),
            (b"\xff\xfb\x90\0", Sniff::Kind(AssetKind::Audio)),
            (b"\0\x01\0\0\0\x10", Sniff::Kind(AssetKind::Font)),
            (b"wOFF\0\x01", Sniff::Kind(AssetKind::Font)),
            (b"glTF\x02\0\0\0", Sniff::Kind(AssetKind::Model)),
            (b"#?RADIANCE\n", Sniff::Kind(AssetKind::Environment)),
            (b"\xff\xe0\0\0", Sniff::Unknown),
            (b"just some text", Sniff::Unknown),
        ];
        for (head, expected) in cases {
            assert_eq!(sniff(head), expected, "{head:?}");
        }
    }

    #[test]
    fn a_container_of_streams_is_not_given_a_kind() {
        let mp4 = sniff(b"\0\0\0\x18ftypmp42");
        assert_eq!(mp4.other_than(AssetKind::Image), Some(None));
        assert_eq!(mp4.other_than(AssetKind::Video), None);
        assert_eq!(mp4.other_than(AssetKind::Audio), None);
        let png = sniff(b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            png.other_than(AssetKind::Video),
            Some(Some(AssetKind::Image))
        );
        assert_eq!(png.other_than(AssetKind::Image), None);
        assert_eq!(sniff(b"text").other_than(AssetKind::Image), None);
    }
}
