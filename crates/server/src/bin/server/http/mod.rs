mod auth;
mod errors;
mod media;
mod routes;
mod state;
mod ws;

use std::{net::SocketAddr, num::NonZeroUsize, path::PathBuf, time::Duration};

use anyhow::bail;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post, put},
};
use clap::Parser;
use lumen_server::render::{
    FONT_DIR_ENV, GpuSettings, HARDWARE_DECODE_ENV, MAX_ARCHIVE_FRAMES, RenderConfig,
    existing_font_dir, is_enabled_flag,
};

use self::{
    auth::require_token,
    media::{
        complete_multipart, create_media, create_url_media, delete_media, get_content, get_media,
        put_content, put_part, start_multipart,
    },
    routes::{
        cancel_render, check_render, create_render, get_render, get_render_artifact,
        get_render_progress, health, render_socket,
    },
    state::AppState,
};

/// Largest JSON body. Compositions reference media by id or through the manifest, so they stay
/// small. Media bytes have their own limit, `--media-max-bytes`.
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(version, about = "Run a provider-neutral Lumen render HTTP server.")]
pub struct Args {
    /// Address to bind the HTTP server to.
    #[arg(long, env = "LUMEN_SERVER_BIND", default_value = "127.0.0.1:8080")]
    pub bind: SocketAddr,

    /// Bearer token required on every route except /health. A non-loopback --bind requires one.
    #[arg(long, env = "LUMEN_SERVER_TOKEN")]
    pub token: Option<String>,

    /// Renders that may run at once. Later renders stay queued until a slot frees.
    #[arg(long, env = "LUMEN_SERVER_MAX_CONCURRENT_RENDERS", default_value = "1")]
    pub max_concurrent_renders: NonZeroUsize,

    /// Renders that may wait for a slot at once. Past it, `POST /renders` answers `503` until one
    /// starts or is cancelled. `0` queues nothing: a render is refused unless a slot is free.
    #[arg(long, env = "LUMEN_SERVER_MAX_QUEUED_RENDERS", default_value = "32")]
    pub max_queued_renders: usize,

    /// Finished renders kept in memory. Beyond this, the oldest record and artifact are dropped.
    #[arg(long, env = "LUMEN_SERVER_RETAINED_RENDERS", default_value = "16")]
    pub retained_renders: NonZeroUsize,

    /// Media host allowed to resolve to non-public addresses, as written in media URLs. Repeatable.
    #[arg(
        long = "allow-media-host",
        env = "LUMEN_SERVER_ALLOW_MEDIA_HOSTS",
        value_delimiter = ','
    )]
    pub allowed_media_hosts: Vec<String>,

    /// Minimum progress increase, from 0 to 1, before sockets get another progress event.
    #[arg(
        long,
        env = "LUMEN_SERVER_PROGRESS_MIN_DELTA",
        default_value_t = 0.02,
        value_parser = parse_fraction
    )]
    pub progress_min_delta: f32,

    /// Most glyph, stroke and decoration instances one text node may draw. A text over it fails
    /// its render, so this bounds the work a composition can ask of its text.
    #[arg(
        long,
        env = "LUMEN_SERVER_MAX_TEXT_GLYPHS",
        default_value_t = lumen_engine::gpu::DEFAULT_MAX_TEXT_GLYPHS as u32,
        value_parser = clap::value_parser!(u32).range(1..=lumen_engine::gpu::MAX_TEXT_GLYPHS_CEILING as i64)
    )]
    pub max_text_glyphs: u32,

    /// Most frames an image sequence render may have. A longer range is refused with `422` and
    /// the code `output_too_large` before any work is done.
    #[arg(
        long,
        env = "LUMEN_SERVER_MAX_SEQUENCE_FRAMES",
        default_value_t = 3600,
        value_parser = clap::value_parser!(u32).range(1..=MAX_ARCHIVE_FRAMES as i64)
    )]
    pub max_sequence_frames: u32,

    /// Most bytes the encoded frames of an image sequence may add up to. A sequence that passes
    /// it ends as `failed` with the code `output_too_large`. The archive is kept in memory like
    /// every artifact, so this bounds what each retained sequence takes.
    #[arg(
        long,
        env = "LUMEN_SERVER_MAX_SEQUENCE_BYTES",
        default_value_t = 512 * 1024 * 1024,
        value_parser = clap::value_parser!(u64).range(1..=3 * 1024 * 1024 * 1024)
    )]
    pub max_sequence_bytes: u64,

    /// JSON file of quality presets and encoder orders, checked at startup. Without it the
    /// built-in presets apply.
    #[arg(long, env = "LUMEN_QUALITY_CONFIG", value_name = "PATH")]
    pub quality_config: Option<PathBuf>,

    /// Directory uploaded media is kept in, created if missing. Media in it survives a restart
    /// until it expires.
    #[arg(
        long,
        env = "LUMEN_SERVER_MEDIA_DIR",
        value_name = "PATH",
        default_value_os_t = std::env::temp_dir().join("lumen-server-media")
    )]
    pub media_dir: PathBuf,

    /// How long uploaded media is kept, as a number of seconds or with an `s`, `m`, `h` or `d`
    /// suffix. Expired media is deleted, except while a render that references it is queued or
    /// running.
    #[arg(
        long,
        env = "LUMEN_SERVER_MEDIA_TTL",
        default_value = "24h",
        value_parser = parse_duration
    )]
    pub media_ttl: Duration,

    /// Largest uploaded or imported media file, in bytes.
    #[arg(
        long,
        env = "LUMEN_SERVER_MEDIA_MAX_BYTES",
        default_value_t = 2 * 1024 * 1024 * 1024
    )]
    pub media_max_bytes: u64,

    /// Most media waiting for its content at once: created with `POST /media` and not yet
    /// checked, deleted or expired. Past it, `POST /media` answers `503` until one of them ends.
    #[arg(long, env = "LUMEN_SERVER_MEDIA_MAX_PENDING", default_value = "100")]
    pub media_max_pending: NonZeroUsize,

    /// Directory of fonts, looked up before the system fonts. Must exist.
    #[arg(long, env = FONT_DIR_ENV, value_name = "PATH", value_parser = existing_font_dir)]
    pub font_dir: Option<PathBuf>,
}

impl Args {
    /// What renders take from this host: the font directory, and `LUMEN_HARDWARE_DECODE` and the
    /// GPU settings from the environment.
    fn render_config(&self) -> RenderConfig {
        RenderConfig {
            font_dir: self
                .font_dir
                .clone()
                .filter(|dir| !dir.as_os_str().is_empty()),
            hardware_decode: std::env::var(HARDWARE_DECODE_ENV)
                .is_ok_and(|value| is_enabled_flag(&value)),
            gpu: GpuSettings::from_env(),
            temp_dir: Some(std::env::temp_dir()),
        }
    }
}

fn parse_duration(raw: &str) -> Result<Duration, String> {
    let invalid =
        || format!("expected a duration such as `90s`, `30m`, `24h` or `7d`, got `{raw}`");
    let (number, unit) = match raw.find(|ch: char| !ch.is_ascii_digit()) {
        Some(index) => raw.split_at(index),
        None => (raw, "s"),
    };
    let seconds_per_unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => return Err(invalid()),
    };
    match number.parse::<u64>() {
        Ok(count) if count > 0 => count
            .checked_mul(seconds_per_unit)
            .map(Duration::from_secs)
            .ok_or_else(invalid),
        _ => Err(invalid()),
    }
}

fn parse_fraction(raw: &str) -> Result<f32, String> {
    match raw.parse::<f32>() {
        Ok(value) if (0.0..=1.0).contains(&value) => Ok(value),
        _ => Err(format!("expected a number from 0 to 1, got `{raw}`")),
    }
}

pub async fn run(mut args: Args) -> anyhow::Result<()> {
    init_tracing();
    args.token.take_if(|token| token.is_empty());
    if args.token.is_none() && !args.bind.ip().to_canonical().is_loopback() {
        bail!(
            "refusing to serve {} without a token: set --token or LUMEN_SERVER_TOKEN, or bind a loopback address",
            args.bind
        );
    }
    let bind = args.bind;
    let render_config = args.render_config();
    let state = AppState::new(args, render_config)?;
    spawn_media_sweep(&state);
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(address = %listener.local_addr()?, "lumen render server listening");
    axum::serve(listener, app).await?;

    Ok(())
}

fn spawn_media_sweep(state: &AppState) {
    let assets = state.assets.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(assets.sweep_interval());
        loop {
            interval.tick().await;
            let assets = assets.clone();
            let _ = tokio::task::spawn_blocking(move || assets.sweep()).await;
        }
    });
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/media", post(create_media))
        .route("/media/url", post(create_url_media))
        .route("/media/{id}", get(get_media).delete(delete_media))
        .route("/media/{id}/content", put(put_content).get(get_content))
        .route("/media/{id}/multipart", post(start_multipart))
        .route(
            "/media/{id}/multipart/{upload_id}/parts/{part_number}",
            put(put_part),
        )
        .route(
            "/media/{id}/multipart/{upload_id}/complete",
            post(complete_multipart),
        )
        .route("/renders", post(create_render))
        .route("/renders/check", post(check_render))
        .route("/renders/{id}", get(get_render))
        .route("/renders/{id}/cancel", post(cancel_render))
        .route("/renders/{id}/progress", get(get_render_progress))
        .route("/renders/{id}/socket", get(render_socket))
        .route("/renders/{id}/artifact", get(get_render_artifact))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        .route("/health", get(health))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn(errors::json_errors))
        .with_state(state)
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "lumen_server=info,warn".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();
}
