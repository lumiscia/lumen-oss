//! Lumen compositing engine crate.

extern crate self as lumen_engine;

use std::sync::atomic::{AtomicU8, Ordering};

use crate::error::LumenError;

#[cfg(all(feature = "ffmpeg", target_arch = "wasm32", target_os = "unknown"))]
compile_error!("Lumen's ffmpeg feature is native-only; use browser media APIs on wasm.");

pub mod audio;
pub mod composition;
pub mod error;
pub mod expr;
pub mod gpu;
pub mod graph;
pub mod media;
pub mod node;

#[cfg(feature = "ffmpeg")]
pub use media::ffmpeg;

#[cfg(feature = "image")]
pub use media::image;

#[cfg(feature = "json")]
pub mod json;

pub(crate) type Result<T> = std::result::Result<T, LumenError>;

static LOG_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Off as u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LogLevel {
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl LogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" | "OFF" | "0" => Some(Self::Off),
            "error" | "ERROR" => Some(Self::Error),
            "warn" | "WARN" | "warning" | "WARNING" => Some(Self::Warn),
            "info" | "INFO" => Some(Self::Info),
            "debug" | "DEBUG" => Some(Self::Debug),
            "trace" | "TRACE" | "1" | "true" | "TRUE" => Some(Self::Trace),
            _ => None,
        }
    }

    fn enables(self, level: tracing::Level) -> bool {
        (self as u8)
            >= match level {
                tracing::Level::ERROR => Self::Error as u8,
                tracing::Level::WARN => Self::Warn as u8,
                tracing::Level::INFO => Self::Info as u8,
                tracing::Level::DEBUG => Self::Debug as u8,
                tracing::Level::TRACE => Self::Trace as u8,
            }
    }
}

pub fn set_log_level(level: LogLevel) {
    LOG_LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn set_log_level_from_str(level: &str) -> std::result::Result<(), String> {
    let Some(level) = LogLevel::parse(level) else {
        return Err(format!(
            "invalid log level '{level}', expected off, error, warn, info, debug, or trace"
        ));
    };
    set_log_level(level);
    Ok(())
}

pub fn log_level_enabled(level: tracing::Level) -> bool {
    let configured = match LOG_LEVEL.load(Ordering::Relaxed) {
        0 => LogLevel::Off,
        1 => LogLevel::Error,
        2 => LogLevel::Warn,
        3 => LogLevel::Info,
        4 => LogLevel::Debug,
        _ => LogLevel::Trace,
    };
    configured.enables(level)
}
