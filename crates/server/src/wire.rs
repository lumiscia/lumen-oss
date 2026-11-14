//! The engine's render types as the HTTP API's wire types. `lumen-server-api` does not depend on
//! the engine, so each keeps its own definition; the matches here stop a field or an engine
//! variant from being added to one side only, and `lumen-generators` checks that each pair has the
//! same JSON schema.

use lumen_engine::render;
use lumen_server_api as api;

pub(crate) fn render_output(output: api::RenderOutput) -> render::RenderOutput {
    let api::RenderOutput {
        format,
        frames,
        jpeg_quality,
    } = output;
    render::RenderOutput {
        format: output_format(format),
        frames: frame_range(frames),
        jpeg_quality,
    }
}

fn frame_range(range: api::FrameRange) -> render::FrameRange {
    let api::FrameRange { start, end } = range;
    render::FrameRange { start, end }
}

/// The engine format `format` names. The wire enum is `#[non_exhaustive]`, so this goes through
/// [`wire_output_format`], whose match the compiler checks.
fn output_format(format: api::OutputFormat) -> render::OutputFormat {
    render::OutputFormat::ALL
        .into_iter()
        .find(|&engine| wire_output_format(engine) == format)
        .expect("every wire output format is an engine output format")
}

fn wire_output_format(format: render::OutputFormat) -> api::OutputFormat {
    use render::OutputFormat as Engine;
    match format {
        Engine::Mp4 => api::OutputFormat::Mp4,
        Engine::Mov => api::OutputFormat::Mov,
        Engine::Webm => api::OutputFormat::Webm,
        Engine::Png => api::OutputFormat::Png,
        Engine::Jpeg => api::OutputFormat::Jpeg,
        Engine::PngSequence => api::OutputFormat::PngSequence,
        Engine::JpegSequence => api::OutputFormat::JpegSequence,
    }
}

/// The answer to `POST /renders/check`, with diagnostics as a client sees them.
pub(crate) fn check_report(report: render::CheckReport) -> api::CheckReport {
    let render::CheckReport { diagnostics, ok } = report;
    api::CheckReport {
        ok,
        diagnostics: crate::sanitized_diagnostics(&diagnostics),
    }
}

/// The HTTP API's code for a render that failed with `code`. An engine code has no variant of its
/// own there.
pub(crate) fn error_code(code: render::ErrorCode) -> api::ErrorCode {
    use render::ErrorCode as Engine;
    match code {
        Engine::Engine(code) => api::ErrorCode::Other(code.to_string()),
        Engine::InvalidProjectPayload => api::ErrorCode::InvalidProjectPayload,
        Engine::InvalidQuality => api::ErrorCode::InvalidQuality,
        Engine::InvalidRenderProfile => api::ErrorCode::InvalidRenderProfile,
        Engine::InvalidConfig => api::ErrorCode::InvalidConfig,
        Engine::InvalidOutput => api::ErrorCode::InvalidOutput,
        Engine::OutputTooLarge => api::ErrorCode::OutputTooLarge,
        Engine::RenderFailed => api::ErrorCode::RenderFailed,
        Engine::EncodeFailed => api::ErrorCode::EncodeFailed,
        Engine::OutputIo => api::ErrorCode::OutputIo,
        Engine::Cancelled => api::ErrorCode::RenderCancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wire_output_format_is_an_engine_one() {
        assert_eq!(
            api::OutputFormat::ALL.len(),
            render::OutputFormat::ALL.len()
        );
        for format in api::OutputFormat::ALL {
            let engine = output_format(format);
            assert_eq!(engine.name(), format.name());
            assert_eq!(wire_output_format(engine), format);
        }
    }

    #[test]
    fn render_codes_keep_their_strings_on_the_wire() {
        use render::ErrorCode as Engine;
        for code in [
            Engine::InvalidProjectPayload,
            Engine::InvalidQuality,
            Engine::InvalidRenderProfile,
            Engine::InvalidConfig,
            Engine::InvalidOutput,
            Engine::OutputTooLarge,
            Engine::RenderFailed,
            Engine::EncodeFailed,
            Engine::OutputIo,
            Engine::Cancelled,
            Engine::Engine("media.decode"),
        ] {
            assert_eq!(error_code(code).as_str(), code.as_str());
        }
    }
}
