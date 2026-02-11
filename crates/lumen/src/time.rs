use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TimeError {
    #[error("frame rate numerator must be greater than 0")]
    InvalidFrameRate,
    #[error("frame rate denominator must be greater than 0")]
    InvalidDenominator,
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
