use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt::Display,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::{TryStreamExt, stream};
use reqwest::{
    Url,
    dns::{Addrs, Name, Resolve, Resolving},
    redirect,
};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;

use super::{RenderJobError, util::sanitize_error_message};

const MAX_MEDIA_ENTRIES: usize = 256;
const MAX_REMOTE_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
const MAX_STAGED_MEDIA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MEDIA_REDIRECTS: usize = 5;
const MEDIA_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MEDIA_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MEDIA_DOWNLOAD_CONCURRENCY: usize = 4;
const NON_PUBLIC_HOST: &str =
    "host has no public address; allow it with --allow-media-host if this is intended";

/// Downloads manifest media, refusing non-public addresses on every connection, including
/// redirects, unless the host is listed in `--allow-media-host`.
#[derive(Clone)]
pub struct MediaClient {
    client: reqwest::Client,
    allowed_hosts: Arc<HashSet<String>>,
}

impl MediaClient {
    pub fn new(allowed_hosts: Vec<String>) -> anyhow::Result<Self> {
        let allowed_hosts: Arc<HashSet<String>> = Arc::new(
            allowed_hosts
                .iter()
                .map(|host| host.to_ascii_lowercase())
                .collect(),
        );
        let redirect_allowed_hosts = Arc::clone(&allowed_hosts);
        let client = reqwest::Client::builder()
            .connect_timeout(MEDIA_CONNECT_TIMEOUT)
            .dns_resolver(Arc::new(PublicAddressResolver {
                allowed_hosts: Arc::clone(&allowed_hosts),
            }))
            // A proxy would resolve the media host itself, out of reach of the resolver.
            .no_proxy()
            .redirect(redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= MAX_MEDIA_REDIRECTS {
                    return attempt.error("too many redirects");
                }
                match check_media_url(attempt.url(), &redirect_allowed_hosts) {
                    Ok(()) => attempt.follow(),
                    Err(cause) => attempt.error(cause),
                }
            }))
            .tcp_nodelay(true)
            .timeout(MEDIA_REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            allowed_hosts,
        })
    }

    /// Downloads `url` to `path`, up to `limit` bytes, counting them in `staged_bytes` too when a
    /// job total applies. `alias` names the file in errors.
    pub(super) async fn download(
        &self,
        alias: &str,
        url: &str,
        path: &Path,
        limit: u64,
        staged_bytes: Option<&AtomicU64>,
    ) -> Result<Downloaded, DownloadError> {
        let parsed = Url::parse(url).map_err(|err| {
            media_error(
                "media_download_failed",
                alias,
                None,
                format!("invalid URL: {err}"),
                false,
            )
        })?;
        let host = parsed.host_str().map(str::to_string);
        let host = host.as_deref();
        let failed = |code: &str, cause: &dyn Display, retryable| {
            media_error(code, alias, host, cause, retryable)
        };
        check_media_url(&parsed, &self.allowed_hosts)
            .map_err(|cause| failed("media_download_forbidden_host", &cause, false))?;

        let started = Instant::now();
        let mut response = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(|err| DownloadError {
                non_public_host: is_non_public_host(&err),
                error: failed("media_download_failed", &request_failure_cause(err), true),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(failed(
                "media_download_failed",
                &format!("server returned status {status}"),
                status.is_server_error() || status.as_u16() == 429,
            )
            .into());
        }
        let too_large = || {
            failed(
                "media_download_too_large",
                &format!("file exceeds the {limit} byte limit"),
                false,
            )
        };
        if response
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(too_large().into());
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        let write_failed = |err: std::io::Error| {
            failed("media_stage_failed", &format!("write failed: {err}"), true)
        };
        let mut file = tokio::fs::File::create(path).await.map_err(write_failed)?;
        let mut downloaded = 0_u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| failed("media_download_failed", &request_failure_cause(err), true))?
        {
            let length = chunk.len() as u64;
            downloaded += length;
            if downloaded > limit {
                return Err(too_large().into());
            }
            if let Some(staged_bytes) = staged_bytes
                && staged_bytes.fetch_add(length, Ordering::Relaxed) + length
                    > MAX_STAGED_MEDIA_BYTES
            {
                return Err(failed(
                    "media_download_too_large",
                    &format!("job exceeds the {MAX_STAGED_MEDIA_BYTES} byte total media limit"),
                    false,
                )
                .into());
            }
            file.write_all(&chunk).await.map_err(write_failed)?;
        }
        file.flush().await.map_err(write_failed)?;
        tracing::info!(
            alias,
            media_url_host = host,
            total_ms = started.elapsed().as_millis(),
            bytes = downloaded,
            "downloaded media source"
        );
        Ok(Downloaded {
            bytes: downloaded,
            content_type,
        })
    }
}

pub(super) struct Downloaded {
    pub bytes: u64,
    pub content_type: Option<String>,
}

/// A failed download: the error a render keeps, and whether it failed because the host resolved
/// only to non-public addresses.
pub(crate) struct DownloadError {
    pub error: RenderJobError,
    pub non_public_host: bool,
}

impl From<RenderJobError> for DownloadError {
    fn from(error: RenderJobError) -> Self {
        Self {
            error,
            non_public_host: false,
        }
    }
}

/// The resolver's refusal of a host with no public address, recognised in a request's errors by
/// its type.
#[derive(Debug)]
struct NonPublicHost;

impl Display for NonPublicHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(NON_PUBLIC_HOST)
    }
}

impl Error for NonPublicHost {}

fn is_non_public_host(error: &reqwest::Error) -> bool {
    let mut source = error.source();
    while let Some(next) = source {
        if next.is::<NonPublicHost>() {
            return true;
        }
        source = next.source();
    }
    false
}

struct PublicAddressResolver {
    allowed_hosts: Arc<HashSet<String>>,
}

impl Resolve for PublicAddressResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allowed = self
            .allowed_hosts
            .contains(&name.as_str().to_ascii_lowercase());
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|address| allowed || !is_non_public_ip(address.ip()))
                .collect::<Vec<SocketAddr>>();
            if addresses.is_empty() {
                return Err(Box::new(NonPublicHost) as Box<dyn Error + Send + Sync>);
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

/// Staging always yields a directory of its own, even with an empty manifest, because the render's
/// media store can only read sources inside the directory it is given.
pub(super) async fn stage_remote_media(
    client: &MediaClient,
    media: HashMap<String, String>,
) -> Result<TempDir, RenderJobError> {
    let dir = tempfile::tempdir().map_err(|err| {
        tracing::warn!(error = %err, "failed to create media staging directory");
        RenderJobError {
            code: "media_stage_failed".to_string(),
            message: "failed to create media staging directory".to_string(),
            retryable: true,
        }
    })?;
    let staged_bytes = AtomicU64::new(0);
    stream::iter(media.into_iter().map(Ok))
        .try_for_each_concurrent(MEDIA_DOWNLOAD_CONCURRENCY, |(alias, url)| {
            let path = dir.path().join(&alias);
            let staged_bytes = &staged_bytes;
            async move {
                client
                    .download(
                        &alias,
                        &url,
                        &path,
                        MAX_REMOTE_MEDIA_BYTES,
                        Some(staged_bytes),
                    )
                    .await
                    .map(drop)
                    .map_err(|failure| failure.error)
            }
        })
        .await?;
    Ok(dir)
}

pub(crate) fn validate_media_manifest(
    media: &HashMap<String, String>,
) -> Result<(), RenderJobError> {
    if media.len() > MAX_MEDIA_ENTRIES {
        return Err(RenderJobError {
            code: "media_manifest_too_large".to_string(),
            message: format!("media manifest exceeds the {MAX_MEDIA_ENTRIES} entry limit"),
            retryable: false,
        });
    }
    if media.keys().all(|alias| is_valid_media_alias(alias)) {
        Ok(())
    } else {
        Err(RenderJobError {
            code: "invalid_media_alias".to_string(),
            message: "media aliases must be at most 128 letters, digits, `.`, `_` or `-`, and not only dots"
                .to_string(),
            retryable: false,
        })
    }
}

/// Aliases become file names in the staging directory, so they cannot name a subdirectory, a parent
/// or a symlink target.
fn is_valid_media_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 128
        && alias.chars().any(|ch| ch != '.')
        && alias
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

/// Connections to a literal IP skip the resolver, so those hosts are checked here.
fn check_media_url(url: &Url, allowed_hosts: &HashSet<String>) -> Result<(), &'static str> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("media manifests take direct http and https URLs");
    }
    let host = url.host_str().ok_or("URL has no host")?;
    let literal_ip = host
        .trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .is_ok_and(is_non_public_ip);
    if literal_ip && !allowed_hosts.contains(host) {
        return Err(NON_PUBLIC_HOST);
    }
    Ok(())
}

fn media_error(
    code: &str,
    alias: &str,
    host: Option<&str>,
    cause: impl Display,
    retryable: bool,
) -> RenderJobError {
    let message = match host {
        Some(host) => format!("media '{alias}' ({host}): {cause}"),
        None => format!("media '{alias}': {cause}"),
    };
    RenderJobError {
        code: code.to_string(),
        message: sanitize_error_message(&message),
        retryable,
    }
}

/// The URL is dropped because it may carry signed query parameters. The cause is only in the source
/// chain, not in reqwest's own message.
fn request_failure_cause(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut cause = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        cause.push_str(": ");
        cause.push_str(&next.to_string());
        source = next.source();
    }
    cause
}

fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => is_non_public_ipv4(ip),
        IpAddr::V6(ip) => is_non_public_ipv6(ip),
    }
}

/// Refuses the IANA IPv4 special-purpose blocks, except the globally reachable AS112 and AMT ones.
fn is_non_public_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, third, _] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || first == 0
        || first >= 224
        // Shared address space, IETF protocol assignments, the 6to4 relay and benchmarking.
        || (first == 100 && (second & 0xc0) == 64)
        || (first == 192 && second == 0 && third == 0)
        || (first == 192 && second == 88 && third == 99)
        || (first == 198 && (second & 0xfe) == 18)
}

/// Global unicast is only allocated from `2000::/3`, so everything else is refused, including the
/// NAT64 prefixes, discard, SRv6, unique-local, link-local and multicast blocks.
fn is_non_public_ipv6(ip: Ipv6Addr) -> bool {
    let [first, second, ..] = ip.segments();
    (first & 0xe000) != 0x2000
        // IETF protocol assignments, which include Teredo, and documentation.
        || (first == 0x2001 && (second < 0x0200 || second == 0x0db8))
        // 6to4, which reaches arbitrary IPv4 addresses.
        || first == 0x2002
        // Documentation.
        || (first == 0x3fff && second < 0x1000)
}

#[cfg(test)]
mod tests {
    use super::{is_non_public_ip, is_valid_media_alias};

    #[test]
    fn non_public_addresses_are_refused_in_every_form() {
        for address in [
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "240.0.0.1",
            "224.0.0.1",
            "::",
            "::1",
            "::169.254.169.254",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:0:169.254.169.254",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::a9fe:a9fe",
            "64:ff9b:1:ffff::1",
            "100::1",
            "100:0:0:1::1",
            "2001::1",
            "2001:1::1",
            "2001:2::1",
            "2001:1ff:ffff::1",
            "2001:db8::1",
            "2002:a9fe:a9fe::",
            "3fff::1",
            "3fff:fff:ffff::1",
            "4000::1",
            "5f00::1",
            "fc00::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
        ] {
            assert!(
                is_non_public_ip(address.parse().unwrap()),
                "{address} should be refused"
            );
        }
        for address in [
            "1.1.1.1",
            "100.128.0.1",
            "192.31.196.1",
            "192.175.48.1",
            "198.20.0.1",
            "::ffff:1.1.1.1",
            "2001:200::1",
            "2606:4700:4700::1111",
            "2620:4f:8000::1",
            "3fff:1000::1",
        ] {
            assert!(
                !is_non_public_ip(address.parse().unwrap()),
                "{address} should be allowed"
            );
        }
    }

    #[test]
    fn aliases_cannot_leave_the_staging_directory() {
        for alias in ["", ".", "..", "a/b", "../hero", "a\\b"] {
            assert!(!is_valid_media_alias(alias), "{alias:?} should be refused");
        }
        for alias in ["hero", "hero.final-2_v1.png", ".hidden"] {
            assert!(is_valid_media_alias(alias), "{alias:?} should be allowed");
        }
    }
}
