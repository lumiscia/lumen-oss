use std::fmt::Display;

use super::codec::{BitDepth, EncoderInfo, encoder_info};
use super::quality::EncoderQuality;

/// An encoder a render may use, with the settings it runs with.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EncoderCandidate {
    pub encoder: String,
    pub quality: EncoderQuality,
}

/// The encoders a render tries, in order, until one opens, and the bit depth every one of them
/// encodes at. Never empty. A pinned encoder is a single candidate, so it either opens or fails
/// the render.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VideoEncoderChoice {
    candidates: Vec<EncoderCandidate>,
    bit_depth: BitDepth,
}

impl VideoEncoderChoice {
    /// 8-bit, or `None` when `candidates` is empty.
    pub fn new(candidates: Vec<EncoderCandidate>) -> Option<Self> {
        (!candidates.is_empty()).then_some(Self {
            candidates,
            bit_depth: BitDepth::Eight,
        })
    }

    /// Exactly `encoder`, with no fallback, at 8 bits.
    pub fn single(encoder: impl Into<String>, quality: EncoderQuality) -> Self {
        Self {
            candidates: vec![EncoderCandidate {
                encoder: encoder.into(),
                quality,
            }],
            bit_depth: BitDepth::Eight,
        }
    }

    /// This choice at `bit_depth`. An encoder that can't write it fails to open.
    pub fn with_bit_depth(mut self, bit_depth: BitDepth) -> Self {
        self.bit_depth = bit_depth;
        self
    }

    pub fn bit_depth(&self) -> BitDepth {
        self.bit_depth
    }

    pub fn candidates(&self) -> &[EncoderCandidate] {
        &self.candidates
    }

    /// The encoder tried first.
    pub fn first(&self) -> &EncoderCandidate {
        &self.candidates[0]
    }

    /// Looks the first encoder up in this FFmpeg build, so a bad name fails before a render does
    /// any work. Encoders the build lacks are skipped with a warning while another remains.
    pub fn look_up(mut self, mut warn: impl FnMut(&str)) -> crate::Result<(Self, EncoderInfo)> {
        loop {
            match encoder_info(&self.first().encoder) {
                Ok(info) => return Ok((self, info)),
                Err(error) if self.candidates.len() > 1 => {
                    warn(&fallback_warning(
                        &self.candidates[0],
                        &self.candidates[1],
                        &error.message,
                    ));
                    self.candidates.remove(0);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Opens the encoders with `open` in order until one starts, warning once for each that
    /// won't, and returns what opened with the encoder's name. The last failure stands. A failure
    /// after this point can't fall back, because frames have already been written.
    pub fn open<T, E: Display>(
        &self,
        mut open: impl FnMut(&EncoderCandidate) -> Result<T, E>,
        mut warn: impl FnMut(&str),
    ) -> Result<(T, String), E> {
        let mut candidates = self.candidates.iter().peekable();
        loop {
            let candidate = candidates.next().expect("a choice has a candidate");
            match open(candidate) {
                Ok(opened) => return Ok((opened, candidate.encoder.clone())),
                Err(error) => match candidates.peek() {
                    Some(next) => warn(&fallback_warning(candidate, next, &error)),
                    None => return Err(error),
                },
            }
        }
    }
}

fn fallback_warning(
    failed: &EncoderCandidate,
    next: &EncoderCandidate,
    reason: impl Display,
) -> String {
    format!(
        "{} is unavailable, encoding with {} instead: {reason}",
        failed.encoder, next.encoder
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn choice(encoders: &[&str]) -> VideoEncoderChoice {
        VideoEncoderChoice::new(
            encoders
                .iter()
                .map(|encoder| EncoderCandidate {
                    encoder: encoder.to_string(),
                    quality: EncoderQuality::default(),
                })
                .collect(),
        )
        .unwrap()
    }

    /// Opens with `open`, returning the result, the names tried and the warnings shown.
    fn run(
        choice: &VideoEncoderChoice,
        open: impl Fn(&str) -> Result<(), String>,
    ) -> (Result<String, String>, Vec<String>, Vec<String>) {
        let tried = RefCell::new(Vec::new());
        let mut warnings = Vec::new();
        let opened = choice
            .open(
                |candidate| {
                    tried.borrow_mut().push(candidate.encoder.clone());
                    open(&candidate.encoder)
                },
                |warning| warnings.push(warning.to_string()),
            )
            .map(|(_, name)| name);
        (opened, tried.into_inner(), warnings)
    }

    #[test]
    fn opening_falls_through_with_one_warning_per_failure() {
        let (opened, tried, warnings) = run(
            &choice(&["hevc_nvenc", "hevc_vulkan", "libx265"]),
            |name| match name {
                "libx265" => Ok(()),
                _ => Err(format!("{name}: no device")),
            },
        );
        assert_eq!(opened.as_deref(), Ok("libx265"));
        assert_eq!(tried, ["hevc_nvenc", "hevc_vulkan", "libx265"]);
        assert_eq!(
            warnings,
            [
                "hevc_nvenc is unavailable, encoding with hevc_vulkan instead: hevc_nvenc: no device",
                "hevc_vulkan is unavailable, encoding with libx265 instead: hevc_vulkan: no device",
            ]
        );
    }

    #[test]
    fn the_first_encoder_that_opens_wins() {
        let (opened, tried, warnings) = run(&choice(&["h264_vulkan", "libx264"]), |_| Ok(()));
        assert_eq!(opened.as_deref(), Ok("h264_vulkan"));
        assert_eq!(tried, ["h264_vulkan"]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn the_last_failure_is_the_error() {
        let (opened, _, warnings) = run(&choice(&["h264_vulkan", "libx264"]), |name| {
            Err(format!("{name} failed"))
        });
        assert_eq!(opened, Err("libx264 failed".to_string()));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_single_encoder_never_falls_back() {
        let (opened, tried, warnings) = run(
            &VideoEncoderChoice::single("h264_vulkan", EncoderQuality::default()),
            |_| Err("no encode queue".to_string()),
        );
        assert_eq!(opened, Err("no encode queue".to_string()));
        assert_eq!(tried, ["h264_vulkan"]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn look_up_skips_encoders_this_build_lacks() {
        let mut warnings = Vec::new();
        let (looked_up, info) = choice(&["missing_encoder", "libx264"])
            .look_up(|warning| warnings.push(warning.to_string()))
            .unwrap();
        assert_eq!(looked_up.candidates().len(), 1);
        assert_eq!(looked_up.first().encoder, "libx264");
        assert_eq!(info.codec, crate::VideoCodec::H264);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("missing_encoder is unavailable"));

        let error = VideoEncoderChoice::single("missing_encoder", EncoderQuality::default())
            .look_up(|warning| panic!("unexpected warning: {warning}"))
            .unwrap_err();
        assert!(error.message.contains("missing_encoder"), "{error}");
    }
}
