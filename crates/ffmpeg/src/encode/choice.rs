use std::{env, fmt::Display};

use super::codec::{EncoderInfo, encoder_info};
use super::quality::EncoderQuality;

const ENCODER_ENV: &str = "LUMEN_VIDEO_ENCODER";
const CPU_ENCODER: &str = "libx264";

/// The video encoder a render uses. Only the automatic Linux default has a fallback: an encoder
/// someone named either opens or fails the render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoEncoderChoice {
    pub name: String,
    fallback: Option<&'static str>,
}

impl VideoEncoderChoice {
    /// Picks `requested`, then `LUMEN_VIDEO_ENCODER`, then the platform default. Blank names count
    /// as unset.
    pub fn resolve(requested: Option<&str>) -> Self {
        Self::from_requests(requested, env::var(ENCODER_ENV).ok().as_deref())
    }

    fn from_requests(requested: Option<&str>, from_env: Option<&str>) -> Self {
        let named = [requested, from_env]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|name| !name.is_empty());
        let (name, fallback) = match named {
            Some(name) => (name, None),
            None if cfg!(target_os = "macos") => ("h264_videotoolbox", None),
            None if cfg!(target_os = "linux") => ("h264_vulkan", Some(CPU_ENCODER)),
            None => (CPU_ENCODER, None),
        };
        Self {
            name: name.to_string(),
            fallback,
        }
    }

    /// Looks the encoder up in this FFmpeg build, so a bad name fails before a render does any work.
    /// The automatic default moves to its fallback when the build doesn't have it.
    pub fn look_up(self, warn: impl FnOnce(&str)) -> crate::Result<(Self, EncoderInfo)> {
        let (info, name) = self.open(encoder_info, warn)?;
        let fallback = if name == self.name {
            self.fallback
        } else {
            None
        };
        Ok((Self { name, fallback }, info))
    }

    /// Whether this choice can encode `quality`, without opening anything. The automatic default
    /// passes when its fallback can, since opening falls back when the quality is unsupported.
    pub fn check_quality(&self, quality: &EncoderQuality) -> Result<(), String> {
        quality
            .check_encoder(&self.name)
            .or_else(|error| match self.fallback {
                Some(fallback) => quality.check_encoder(fallback),
                None => Err(error),
            })
    }

    /// The choice to continue with after this encoder failed to open, and the warning to show for
    /// it. `None` means the failure stands.
    pub fn fall_back(&self, reason: impl Display) -> Option<(Self, String)> {
        let fallback = self.fallback?;
        let warning = format!(
            "{} is unavailable, encoding with {fallback} on the CPU instead: {reason}",
            self.name
        );
        let choice = Self {
            name: fallback.to_string(),
            fallback: None,
        };
        Some((choice, warning))
    }

    /// Opens the encoder with `open`, moving to the fallback if it won't start, and returns it with
    /// the name of the encoder that opened. A failure after this point can't fall back, because
    /// frames have already been written.
    pub fn open<T, E: Display>(
        &self,
        mut open: impl FnMut(&str) -> Result<T, E>,
        warn: impl FnOnce(&str),
    ) -> Result<(T, String), E> {
        match open(&self.name) {
            Ok(opened) => Ok((opened, self.name.clone())),
            Err(error) => {
                let Some((fallback, warning)) = self.fall_back(&error) else {
                    return Err(error);
                };
                warn(&warning);
                Ok((open(&fallback.name)?, fallback.name))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Opens with `open`, returning the result, the names tried and the warnings shown.
    fn run(
        choice: &VideoEncoderChoice,
        open: impl Fn(&str) -> Result<(), String>,
    ) -> (Result<String, String>, Vec<String>, Vec<String>) {
        let tried = RefCell::new(Vec::new());
        let mut warnings = Vec::new();
        let opened = choice
            .open(
                |name| {
                    tried.borrow_mut().push(name.to_string());
                    open(name)
                },
                |warning| warnings.push(warning.to_string()),
            )
            .map(|(_, name)| name);
        (opened, tried.into_inner(), warnings)
    }

    #[test]
    fn names_win_over_the_environment_and_the_default() {
        let chosen = |requested, from_env| VideoEncoderChoice::from_requests(requested, from_env);
        assert_eq!(
            chosen(Some(" libx265 "), Some("h264_nvenc")).name,
            "libx265"
        );
        assert_eq!(chosen(Some("  "), Some("h264_nvenc")).name, "h264_nvenc");
        assert_eq!(chosen(Some(""), Some(" ")), chosen(None, None));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_default_warns_once_with_the_reason_and_uses_libx264() {
        let (opened, tried, warnings) =
            run(&VideoEncoderChoice::from_requests(None, None), |name| {
                if name == "h264_vulkan" {
                    Err("no encode queue".to_string())
                } else {
                    Ok(())
                }
            });
        assert_eq!(opened.as_deref(), Ok("libx264"));
        assert_eq!(tried, ["h264_vulkan", "libx264"]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("no encode queue"), "{warnings:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_default_stays_on_h264_vulkan_when_it_opens() {
        let (opened, tried, warnings) =
            run(&VideoEncoderChoice::from_requests(None, None), |_| Ok(()));
        assert_eq!(opened.as_deref(), Ok("h264_vulkan"));
        assert_eq!(tried, ["h264_vulkan"]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn the_fallback_failing_reports_its_own_error() {
        let (opened, _, _) = run(&VideoEncoderChoice::from_requests(None, None), |name| {
            Err(format!("{name} failed"))
        });
        let default = VideoEncoderChoice::from_requests(None, None);
        let expected = if default.fallback.is_some() {
            "libx264 failed".to_string()
        } else {
            format!("{} failed", default.name)
        };
        assert_eq!(opened, Err(expected));
    }

    #[test]
    fn a_default_this_build_lacks_moves_to_the_fallback_at_look_up() {
        let default = VideoEncoderChoice {
            name: "missing_encoder".to_string(),
            fallback: Some(CPU_ENCODER),
        };
        let mut warnings = Vec::new();
        let (choice, info) = default
            .look_up(|warning| warnings.push(warning.to_string()))
            .unwrap();
        assert_eq!(choice.name, CPU_ENCODER);
        assert_eq!(info.codec, crate::VideoCodec::H264);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(choice.fall_back("no").is_none());
    }

    #[test]
    fn a_named_encoder_this_build_lacks_errors_at_look_up() {
        let named = VideoEncoderChoice::from_requests(Some("missing_encoder"), None);
        let error = named
            .look_up(|warning| panic!("unexpected warning: {warning}"))
            .unwrap_err();
        assert!(error.message.contains("missing_encoder"), "{error}");
    }

    #[test]
    fn quality_checks_the_fallback_only_for_the_default() {
        let crf = EncoderQuality {
            rate_control: crate::RateControl::Crf(20),
            preset: Some("slow".to_string()),
        };
        let named = VideoEncoderChoice::from_requests(None, Some("h264_vulkan"));
        assert!(
            named
                .check_quality(&crf)
                .unwrap_err()
                .contains("h264_vulkan")
        );
        let default = VideoEncoderChoice {
            name: "h264_vulkan".to_string(),
            fallback: Some(CPU_ENCODER),
        };
        assert_eq!(default.check_quality(&crf), Ok(()));
    }

    #[test]
    fn named_encoders_never_fall_back() {
        for choice in [
            VideoEncoderChoice::from_requests(Some("h264_vulkan"), None),
            VideoEncoderChoice::from_requests(None, Some("h264_vulkan")),
        ] {
            let (opened, tried, warnings) = run(&choice, |_| Err("no encode queue".to_string()));
            assert_eq!(opened, Err("no encode queue".to_string()));
            assert_eq!(tried, ["h264_vulkan"]);
            assert!(warnings.is_empty(), "{warnings:?}");
            assert!(choice.fall_back("no encode queue").is_none());
        }
    }
}
