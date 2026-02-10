use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::EnvFilter;

use crate::server::serve;

mod api_error;
mod app_state;
mod codecs;
mod endpoint;
mod jobs;
mod middleware;
mod server;
#[cfg(test)]
mod tests;
mod video;

const AV_LOG_PANIC: i32 = 0;
const AV_LOG_FATAL: i32 = 8;
const AV_LOG_ERROR: i32 = 16;
const AV_LOG_WARNING: i32 = 24;
const AV_LOG_INFO: i32 = 32;
const AV_LOG_VERBOSE: i32 = 40;
const AV_LOG_DEBUG: i32 = 48;
const AV_LOG_TRACE: i32 = 56;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt().with_env_filter(filter).init(); // Initialize the global tracing subscriber

    ac_ffmpeg::set_log_callback(log_callback);

    if let Err(err) = serve().await {
        error!("{err:?}");
    };
}

pub fn log_callback(level: i32, message: &str) {
    match level {
        l if l <= AV_LOG_PANIC => error!(target: "ffmpeg", "[panic] {}", message.trim_end()),
        l if l <= AV_LOG_FATAL => error!(target: "ffmpeg", "[fatal] {}", message.trim_end()),
        l if l <= AV_LOG_ERROR => error!(target: "ffmpeg", "{}", message.trim_end()),
        l if l <= AV_LOG_WARNING => warn!(target: "ffmpeg", "{}", message.trim_end()),
        l if l <= AV_LOG_INFO => info!(target: "ffmpeg", "{}", message.trim_end()),
        l if l <= AV_LOG_VERBOSE => debug!(target: "ffmpeg", "{}", message.trim_end()),
        l if l <= AV_LOG_DEBUG => debug!(target: "ffmpeg", "{}", message.trim_end()),
        l if l <= AV_LOG_TRACE => trace!(target: "ffmpeg", "{}", message.trim_end()),
        _ => trace!(target: "ffmpeg", "[unknown level {}] {}", level, message.trim_end()),
    }
}
