use std::{error::Error, fmt};

use lumen_engine::error::LumenError;

type Source = Box<dyn Error + Send + Sync + 'static>;

/// Why a render failed: a [`code`](Self::code) to match on, a message, and the error that caused
/// it as [`source`](Error::source).
#[derive(Debug)]
pub struct ServerRenderError {
    code: ErrorCode,
    message: String,
    retryable: bool,
    source: Option<Source>,
}

/// What a render failed with. An engine failure has the engine's public diagnostic code; the other
/// codes are failures of this library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The engine failed, such as `media.not_found` for a source that isn't there or `render.gpu`
    /// for the GPU. The code is one of [`lumen_engine::diagnostic::codes`].
    Engine(&'static str),
    /// `invalid_project_payload`: the payload is not a composition.
    InvalidProjectPayload,
    /// `invalid_quality`: the quality preset or codec resolves to no encoder.
    InvalidQuality,
    /// `invalid_render_profile`: an encoder that doesn't exist, or a frame rate it can't encode.
    InvalidRenderProfile,
    /// `invalid_config`: the [`RenderConfig`](super::RenderConfig) or options name something that
    /// isn't there, such as a media root or font directory.
    InvalidConfig,
    /// `invalid_output`: the [`RenderOutput`](super::RenderOutput) can't be rendered: an empty
    /// frame range, one past the end of the timeline, a still of several frames, or a JPEG quality
    /// that isn't 1 to 100 or goes with another format.
    InvalidOutput,
    /// `output_too_large`: an image sequence has more frames, or adds up to more bytes, than its
    /// [`SequenceLimits`](super::SequenceLimits) allow.
    OutputTooLarge,
    /// `render_failed`: the GPU work around the engine failed, such as reading a frame back.
    RenderFailed,
    /// `encode_failed`: the encoder failed to open, encode or finish.
    EncodeFailed,
    /// `output_io`: the output couldn't be written or read back.
    OutputIo,
    /// `render_cancelled`: the render's [`CancelToken`](super::CancelToken) was cancelled. It
    /// isn't a failure, and rendering again would do the same work.
    Cancelled,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Engine(code) => code,
            Self::InvalidProjectPayload => "invalid_project_payload",
            Self::InvalidQuality => "invalid_quality",
            Self::InvalidRenderProfile => "invalid_render_profile",
            Self::InvalidConfig => "invalid_config",
            Self::InvalidOutput => "invalid_output",
            Self::OutputTooLarge => "output_too_large",
            Self::RenderFailed => "render_failed",
            Self::EncodeFailed => "encode_failed",
            Self::OutputIo => "output_io",
            Self::Cancelled => "render_cancelled",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ServerRenderError {
    /// A failure with `code` and nothing that caused it.
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            source: None,
        }
    }

    /// The render stopped because its token was cancelled.
    pub(crate) fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "the render was cancelled", false)
    }

    /// An engine failure, while doing what `context` says.
    pub(crate) fn engine(context: impl Into<String>, error: impl Into<LumenError>) -> Self {
        Self::new(ErrorCode::RenderFailed, context, false).caused_by(error.into())
    }

    /// Records `cause` as the source. A cause that is, or wraps, an engine error takes the
    /// engine's code, and is retryable only when the GPU failed or a media source failed on a
    /// shortage that can pass.
    pub(crate) fn caused_by(mut self, cause: impl Into<Source>) -> Self {
        self.source = Some(cause.into());
        if let Some(engine) = self.engine_error() {
            let code = engine.code();
            let transient = engine.is_transient();
            self.code = ErrorCode::Engine(code);
            self.retryable = code == "render.gpu" || transient;
        }
        self
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Whether the same render may succeed when tried again.
    pub fn retryable(&self) -> bool {
        self.retryable
    }

    /// The engine error this failure comes from, if any.
    pub fn engine_error(&self) -> Option<&LumenError> {
        let mut next: Option<&(dyn Error + 'static)> = self.source.as_deref().map(|e| e as _);
        while let Some(error) = next {
            if let Some(engine) = error.downcast_ref::<LumenError>() {
                return Some(engine);
            }
            next = error.source();
        }
        None
    }
}

impl fmt::Display for ServerRenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ServerRenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|error| error as _)
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use lumen_engine::error::{MediaError, RenderError};

    use super::*;

    #[test]
    fn server_codes_are_their_documented_strings() {
        let codes = [
            (ErrorCode::InvalidProjectPayload, "invalid_project_payload"),
            (ErrorCode::InvalidQuality, "invalid_quality"),
            (ErrorCode::InvalidRenderProfile, "invalid_render_profile"),
            (ErrorCode::InvalidConfig, "invalid_config"),
            (ErrorCode::InvalidOutput, "invalid_output"),
            (ErrorCode::OutputTooLarge, "output_too_large"),
            (ErrorCode::RenderFailed, "render_failed"),
            (ErrorCode::EncodeFailed, "encode_failed"),
            (ErrorCode::OutputIo, "output_io"),
            (ErrorCode::Cancelled, "render_cancelled"),
        ];
        for (code, name) in codes {
            assert_eq!(code.as_str(), name);
            assert!(
                lumen_engine::diagnostic::codes().all(|engine| engine != name),
                "{name} is an engine code"
            );
        }
        assert_eq!(ErrorCode::Engine("media.decode").as_str(), "media.decode");
    }

    #[test]
    fn an_engine_failure_keeps_its_code_and_error() {
        let error = ServerRenderError::engine(
            "render failed at frame 3",
            MediaError::source_not_found("hero.png", "no such file in the media root"),
        );

        assert_eq!(error.code(), ErrorCode::Engine("media.not_found"));
        assert!(!error.retryable());
        assert_eq!(error.to_string(), "render failed at frame 3");
        assert_eq!(
            error.source().unwrap().to_string(),
            "media source `hero.png` not found: no such file in the media root"
        );
        assert!(matches!(error.engine_error(), Some(LumenError::Media(_))));
    }

    #[test]
    fn a_media_failure_is_retryable_only_when_it_is_a_shortage_that_can_pass() {
        use lumen_engine::error::MediaFailure;

        let failures = [
            (
                MediaFailure::not_found("a.png", "gone"),
                "media.not_found",
                false,
            ),
            (
                MediaFailure::unsupported("a.mp4", "codec"),
                "media.unsupported",
                false,
            ),
            (
                MediaFailure::too_large("a.glb", "big", 1, Some(2)),
                "media.too_large",
                false,
            ),
            (
                MediaFailure::decode("a.png", "corrupt"),
                "media.decode",
                false,
            ),
            (
                MediaFailure::decode("a.png", "no memory").transient(),
                "media.decode",
                true,
            ),
        ];
        for (failure, code, retryable) in failures {
            let error = ServerRenderError::engine("render failed", MediaError::from(failure));
            assert_eq!(error.code(), ErrorCode::Engine(code));
            assert_eq!(error.retryable(), retryable, "{code}");
        }
    }

    #[test]
    fn an_engine_error_inside_another_cause_sets_the_code() {
        let gpu: anyhow::Result<()> = Err(LumenError::from(RenderError::Gpu {
            details: "device lost".to_string(),
        }))
        .context("failed to initialize wgpu renderer");
        let error = ServerRenderError::new(ErrorCode::RenderFailed, "no renderer", false)
            .caused_by(gpu.unwrap_err());

        assert_eq!(error.code(), ErrorCode::Engine("render.gpu"));
        assert!(error.retryable());
        assert!(error.engine_error().is_some());
    }

    #[test]
    fn a_cause_from_outside_the_engine_keeps_the_server_code() {
        let error = ServerRenderError::new(ErrorCode::EncodeFailed, "encoder failed", true)
            .caused_by(anyhow::anyhow!("libx264 refused the frame"));

        assert_eq!(error.code(), ErrorCode::EncodeFailed);
        assert!(error.retryable());
        assert!(error.engine_error().is_none());
        assert_eq!(
            format!("{:#}", anyhow::Error::new(error)),
            "encoder failed: libx264 refused the frame"
        );
    }
}
