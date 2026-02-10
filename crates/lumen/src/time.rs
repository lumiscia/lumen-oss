use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TimeError {
    #[error("rational denominator must be greater than 0")]
    InvalidDenominator,
    #[error("timescale must be greater than 0")]
    InvalidTimescale,
    #[error("time value must not be negative")]
    NegativeTime,
    #[error("frame rate must be greater than 0")]
    InvalidFrameRate,
    #[error("time conversion overflowed")]
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub fn new(num: u32, den: u32) -> Result<Self, TimeError> {
        if num == 0 {
            return Err(TimeError::InvalidFrameRate);
        }

        if den == 0 {
            return Err(TimeError::InvalidDenominator);
        }

        Ok(Self { num, den })
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Time {
    pub value: i64,
    pub timescale: u32,
}

impl Time {
    pub const ZERO: Self = Self {
        value: 0,
        timescale: 1,
    };

    pub fn new(value: i64, timescale: u32) -> Result<Self, TimeError> {
        if timescale == 0 {
            return Err(TimeError::InvalidTimescale);
        }

        Ok(Self { value, timescale })
    }

    pub fn require_non_negative(self) -> Result<Self, TimeError> {
        if self.value < 0 {
            return Err(TimeError::NegativeTime);
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FrameIndex(pub u64);

pub fn frames_from_time(duration: Time, fps: Rational) -> Result<u64, TimeError> {
    duration.require_non_negative()?;
    validate_fps(fps)?;

    let numerator = (duration.value as i128)
        .checked_mul(fps.num as i128)
        .ok_or(TimeError::Overflow)?;
    let denominator = (duration.timescale as i128)
        .checked_mul(fps.den as i128)
        .ok_or(TimeError::Overflow)?;

    let ceil = ceil_div(numerator, denominator);
    if ceil < 0 || ceil > u64::MAX as i128 {
        return Err(TimeError::Overflow);
    }

    Ok(ceil as u64)
}

pub fn frame_at_time(time: Time, fps: Rational) -> Result<FrameIndex, TimeError> {
    time.require_non_negative()?;
    validate_fps(fps)?;

    let numerator = (time.value as i128)
        .checked_mul(fps.num as i128)
        .ok_or(TimeError::Overflow)?;
    let denominator = (time.timescale as i128)
        .checked_mul(fps.den as i128)
        .ok_or(TimeError::Overflow)?;

    let floor = numerator.checked_div(denominator).ok_or(TimeError::Overflow)?;
    if floor < 0 || floor > u64::MAX as i128 {
        return Err(TimeError::Overflow);
    }

    Ok(FrameIndex(floor as u64))
}

pub fn time_at_frame(frame: FrameIndex, fps: Rational, timescale: u32) -> Result<Time, TimeError> {
    validate_fps(fps)?;
    if timescale == 0 {
        return Err(TimeError::InvalidTimescale);
    }

    let value = (frame.0 as i128)
        .checked_mul(fps.den as i128)
        .ok_or(TimeError::Overflow)?
        .checked_mul(timescale as i128)
        .ok_or(TimeError::Overflow)?
        .checked_div(fps.num as i128)
        .ok_or(TimeError::Overflow)?;

    if value > i64::MAX as i128 {
        return Err(TimeError::Overflow);
    }

    Time::new(value as i64, timescale)
}

fn validate_fps(fps: Rational) -> Result<(), TimeError> {
    if fps.num == 0 {
        return Err(TimeError::InvalidFrameRate);
    }

    if fps.den == 0 {
        return Err(TimeError::InvalidDenominator);
    }

    Ok(())
}

fn ceil_div(numerator: i128, denominator: i128) -> i128 {
    (numerator + denominator - 1) / denominator
}
