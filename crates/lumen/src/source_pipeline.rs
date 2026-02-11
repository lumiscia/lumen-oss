use thiserror::Error;

use crate::model::{LoopMode, SourcePipeline};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PipelineError {
    #[error("pipeline speed must be finite and greater than 0")]
    InvalidSpeed,
    #[error("trim end_frame must be greater than trim start_frame")]
    InvalidTrimRange,
    #[error("reverse playback requires a bounded trim range")]
    ReverseRequiresBoundedTrim,
    #[error("looping requires a bounded trim range")]
    LoopRequiresBoundedTrim,
    #[error("finite loop count must be greater than 0")]
    InvalidLoopCount,
    #[error("source frame mapping overflowed")]
    Overflow,
}

pub fn map_source_frame(
    pipeline: &SourcePipeline,
    local_frame: u64,
) -> Result<Option<u64>, PipelineError> {
    if !pipeline.speed.is_finite() || pipeline.speed <= 0.0 {
        return Err(PipelineError::InvalidSpeed);
    }

    let trim_start = pipeline
        .trim
        .as_ref()
        .map(|trim| trim.start_frame)
        .unwrap_or(0);
    let bounded_span = if let Some(trim) = pipeline.trim {
        if let Some(end_frame) = trim.end_frame {
            if end_frame <= trim.start_frame {
                return Err(PipelineError::InvalidTrimRange);
            }
            Some(end_frame.saturating_sub(trim.start_frame))
        } else {
            None
        }
    } else {
        None
    };

    let source_progress = ((local_frame as f64) * (pipeline.speed as f64)).floor();
    if !source_progress.is_finite() || source_progress < 0.0 {
        return Err(PipelineError::InvalidSpeed);
    }
    if source_progress > (u64::MAX as f64) {
        return Err(PipelineError::Overflow);
    }

    let source_progress = source_progress as u64;

    let mut offset = match pipeline.looping {
        LoopMode::None => {
            if let Some(span) = bounded_span {
                if source_progress >= span {
                    return Ok(None);
                }
            }
            source_progress
        }
        LoopMode::Finite { count } => {
            let span = bounded_span.ok_or(PipelineError::LoopRequiresBoundedTrim)?;
            if span == 0 {
                return Ok(None);
            }
            if count == 0 {
                return Err(PipelineError::InvalidLoopCount);
            }
            let total = span
                .checked_mul(count as u64)
                .ok_or(PipelineError::Overflow)?;
            if source_progress >= total {
                return Ok(None);
            }
            source_progress % span
        }
        LoopMode::Infinite => {
            let span = bounded_span.ok_or(PipelineError::LoopRequiresBoundedTrim)?;
            if span == 0 {
                return Ok(None);
            }
            source_progress % span
        }
    };

    if pipeline.reverse {
        let span = bounded_span.ok_or(PipelineError::ReverseRequiresBoundedTrim)?;
        offset = span
            .saturating_sub(1)
            .saturating_sub(offset.min(span.saturating_sub(1)));
    }

    let source_frame = trim_start
        .checked_add(offset)
        .ok_or(PipelineError::Overflow)?;

    Ok(Some(source_frame))
}

#[cfg(test)]
mod tests {
    use super::map_source_frame;
    use crate::model::{LoopMode, SourcePipeline, TrimRange};

    #[test]
    fn maps_simple_trimmed_stream() {
        let pipeline = SourcePipeline {
            trim: Some(TrimRange {
                start_frame: 10,
                end_frame: Some(20),
            }),
            speed: 1.0,
            reverse: false,
            looping: LoopMode::None,
        };

        assert_eq!(map_source_frame(&pipeline, 0).expect("map"), Some(10));
        assert_eq!(map_source_frame(&pipeline, 9).expect("map"), Some(19));
        assert_eq!(map_source_frame(&pipeline, 10).expect("map"), None);
    }

    #[test]
    fn maps_speed_and_reverse() {
        let pipeline = SourcePipeline {
            trim: Some(TrimRange {
                start_frame: 100,
                end_frame: Some(110),
            }),
            speed: 2.0,
            reverse: true,
            looping: LoopMode::None,
        };

        assert_eq!(map_source_frame(&pipeline, 0).expect("map"), Some(109));
        assert_eq!(map_source_frame(&pipeline, 1).expect("map"), Some(107));
        assert_eq!(map_source_frame(&pipeline, 4).expect("map"), Some(101));
        assert_eq!(map_source_frame(&pipeline, 5).expect("map"), None);
    }

    #[test]
    fn maps_infinite_loop() {
        let pipeline = SourcePipeline {
            trim: Some(TrimRange {
                start_frame: 42,
                end_frame: Some(45),
            }),
            speed: 1.0,
            reverse: false,
            looping: LoopMode::Infinite,
        };

        assert_eq!(map_source_frame(&pipeline, 0).expect("map"), Some(42));
        assert_eq!(map_source_frame(&pipeline, 2).expect("map"), Some(44));
        assert_eq!(map_source_frame(&pipeline, 3).expect("map"), Some(42));
        assert_eq!(map_source_frame(&pipeline, 8).expect("map"), Some(44));
    }
}
