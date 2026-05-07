use std::{
    collections::HashSet,
    env,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{signal, sync::Mutex, task::JoinSet, time};

use crate::server::RenderJobInput;

use super::{RunpodJobRequest, handle_runpod_request};

const RUNPOD_VERSION: &str = "rust-lumen-runpod";
const JOB_FETCH_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_CONCURRENCY: usize = 2;
const MAX_CONCURRENCY: usize = 4;
const POST_RESULT_MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    worker_id: String,
    api_key: Option<String>,
    get_job_url: String,
    post_output_url: String,
    post_stream_url: Option<String>,
    ping_url: Option<String>,
    ping_interval: Duration,
    concurrency: usize,
}

#[derive(Debug, Deserialize)]
struct WorkerJob {
    id: String,
    input: RenderJobInput,
}

pub fn is_runpod_serverless() -> bool {
    env::var_os("RUNPOD_WEBHOOK_GET_JOB").is_some()
}

pub async fn run_serverless_worker() -> Result<()> {
    Worker::from_env()?.run().await
}

struct Worker {
    client: Client,
    config: WorkerConfig,
    active_jobs: Arc<Mutex<HashSet<String>>>,
    shutting_down: Arc<AtomicBool>,
}

impl Worker {
    fn from_env() -> Result<Self> {
        let worker_id = env::var("RUNPOD_POD_ID").unwrap_or_else(|_| uuidish_worker_id());
        let config = WorkerConfig {
            worker_id,
            api_key: env::var("RUNPOD_AI_API_KEY").ok(),
            get_job_url: required_env("RUNPOD_WEBHOOK_GET_JOB")?,
            post_output_url: required_env("RUNPOD_WEBHOOK_POST_OUTPUT")?,
            post_stream_url: env::var("RUNPOD_WEBHOOK_POST_STREAM").ok(),
            ping_url: env::var("RUNPOD_WEBHOOK_PING").ok(),
            ping_interval: env::var("RUNPOD_PING_INTERVAL")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_millis)
                .unwrap_or_else(|| Duration::from_secs(10)),
            concurrency: env::var("LUMEN_RUNPOD_CONCURRENCY")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(DEFAULT_CONCURRENCY)
                .clamp(1, MAX_CONCURRENCY),
        };

        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("failed to build RunPod worker HTTP client")?;

        Ok(Self {
            client,
            config,
            active_jobs: Arc::new(Mutex::new(HashSet::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn run(self) -> Result<()> {
        tracing::info!(
            worker_id = self.config.worker_id,
            concurrency = self.config.concurrency,
            has_stream_url = self.config.post_stream_url.is_some(),
            "starting RunPod serverless worker"
        );

        self.spawn_shutdown_listener();
        self.spawn_heartbeat();

        let mut jobs = JoinSet::new();

        while !self.shutting_down.load(Ordering::Relaxed) {
            while let Some(result) = jobs.try_join_next() {
                if let Err(error) = result {
                    tracing::warn!("RunPod job task failed: {error}");
                }
            }

            let active_count = self.active_jobs.lock().await.len();
            let available_slots = self.config.concurrency.saturating_sub(active_count);
            if available_slots == 0 {
                time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            match self.take_jobs(available_slots).await {
                Ok(fetched_jobs) => {
                    if fetched_jobs.is_empty() {
                        continue;
                    }

                    for job in fetched_jobs {
                        self.active_jobs.lock().await.insert(job.id.clone());
                        let client = self.client.clone();
                        let config = self.config.clone();
                        let active_jobs = Arc::clone(&self.active_jobs);
                        let shutting_down = Arc::clone(&self.shutting_down);

                        jobs.spawn(async move {
                            handle_job(client, config, active_jobs, shutting_down, job).await;
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!("failed to acquire RunPod job: {error:#}");
                    time::sleep(Duration::from_secs(1)).await;
                }
            }
        }

        while let Some(result) = jobs.join_next().await {
            if let Err(error) = result {
                tracing::warn!("RunPod job task failed during drain: {error}");
            }
        }

        tracing::info!("RunPod serverless worker stopped");
        Ok(())
    }

    fn spawn_shutdown_listener(&self) {
        let shutting_down = Arc::clone(&self.shutting_down);
        tokio::spawn(async move {
            let ctrl_c = signal::ctrl_c();

            #[cfg(unix)]
            let terminate = async {
                let mut stream =
                    signal::unix::signal(signal::unix::SignalKind::terminate()).ok()?;
                stream.recv().await
            };

            #[cfg(not(unix))]
            let terminate = std::future::pending::<Option<()>>();

            tokio::select! {
                _ = ctrl_c => {}
                _ = terminate => {}
            }

            shutting_down.store(true, Ordering::Relaxed);
        });
    }

    fn spawn_heartbeat(&self) {
        let Some(ping_url) = self.config.ping_url.clone() else {
            return;
        };
        let client = self.client.clone();
        let config = self.config.clone();
        let active_jobs = Arc::clone(&self.active_jobs);
        let shutting_down = Arc::clone(&self.shutting_down);

        tokio::spawn(async move {
            let mut ticker = time::interval(config.ping_interval);

            while !shutting_down.load(Ordering::Relaxed) {
                ticker.tick().await;
                let url = worker_url(&ping_url, &config.worker_id);
                let job_ids = active_jobs
                    .lock()
                    .await
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",");
                let mut request = client.get(url).query(&[("runpod_version", RUNPOD_VERSION)]);
                if !job_ids.is_empty() {
                    request = request.query(&[("job_id", job_ids)]);
                }

                if let Some(api_key) = &config.api_key {
                    request = request.header("authorization", api_key);
                }

                if let Err(error) = request
                    .send()
                    .await
                    .and_then(|response| response.error_for_status().map(|_| ()))
                {
                    tracing::warn!("failed to send RunPod heartbeat: {error}");
                }
            }
        });
    }

    async fn take_jobs(&self, count: usize) -> Result<Vec<WorkerJob>> {
        let mut url = worker_url(
            &job_get_url(&self.config.get_job_url, count),
            &self.config.worker_id,
        );
        append_query(
            &mut url,
            "job_in_progress",
            if self.active_jobs.lock().await.is_empty() {
                "0"
            } else {
                "1"
            },
        );
        if count > 1 {
            append_query(&mut url, "batch_size", &count.to_string());
        }

        let mut request = self.client.get(url).timeout(JOB_FETCH_TIMEOUT);
        if let Some(api_key) = &self.config.api_key {
            request = request.header("authorization", api_key);
        }

        let response = request.send().await?;
        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::BAD_REQUEST => return Ok(Vec::new()),
            StatusCode::TOO_MANY_REQUESTS => {
                time::sleep(Duration::from_secs(5)).await;
                return Ok(Vec::new());
            }
            _ => {}
        }

        let response = response.error_for_status()?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();

        if !content_type.contains("application/json") {
            return Ok(Vec::new());
        }

        let value = response.json::<Value>().await?;
        if value.is_array() {
            return Ok(serde_json::from_value::<Vec<WorkerJob>>(value)?);
        }

        Ok(vec![serde_json::from_value::<WorkerJob>(value)?])
    }
}

async fn handle_job(
    client: Client,
    config: WorkerConfig,
    active_jobs: Arc<Mutex<HashSet<String>>>,
    shutting_down: Arc<AtomicBool>,
    job: WorkerJob,
) {
    tracing::info!(job_id = job.id, "started RunPod job");

    let response = handle_runpod_request(RunpodJobRequest { input: job.input }).await;
    let response_ok = response.ok;
    let error_code = response.error.as_ref().map(|error| error.code.as_str());
    let job_result = json!({ "output": response });

    let posted_result =
        match post_result(&client, &config, &shutting_down, &job.id, &job_result).await {
            Ok(()) => {
                tracing::info!(
                    job_id = job.id,
                    response_ok,
                    error_code,
                    "posted RunPod job result"
                );
                true
            }
            Err(error) => {
                tracing::error!(
                    job_id = job.id,
                    response_ok,
                    error_code,
                    "failed to post RunPod job result after retries: {error:#}"
                );
                false
            }
        };

    active_jobs.lock().await.remove(&job.id);
    tracing::info!(
        job_id = job.id,
        response_ok,
        error_code,
        posted_result,
        "finished RunPod job task"
    );
}

async fn post_result(
    client: &Client,
    config: &WorkerConfig,
    shutting_down: &AtomicBool,
    job_id: &str,
    result: &Value,
) -> Result<()> {
    let mut url = config.post_output_url.replace("$ID", job_id);
    url = worker_url(&url, &config.worker_id);
    append_query(&mut url, "isStream", "false");
    let body = serde_json::to_string(result)?;

    let mut delay = Duration::from_millis(250);
    let mut attempt = 1_u64;
    while !shutting_down.load(Ordering::Relaxed) {
        tracing::debug!(job_id, attempt, "posting RunPod job result");
        let mut request = client
            .post(url.clone())
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body.clone());

        if let Some(api_key) = &config.api_key {
            request = request.header("authorization", api_key);
        }

        match request
            .send()
            .await
            .and_then(|response| response.error_for_status())
        {
            Ok(_) => return Ok(()),
            Err(error) => {
                tracing::warn!(
                    job_id,
                    attempt,
                    retry_delay_ms = delay.as_millis(),
                    "failed to post RunPod result, keeping job active and retrying: {error}"
                );
                time::sleep(delay).await;
                delay = (delay * 2).min(POST_RESULT_MAX_DELAY);
                attempt += 1;
            }
        }
    }

    anyhow::bail!("worker shut down before RunPod job result could be posted")
}

fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} is required for RunPod serverless worker mode"))
}

fn worker_url(template: &str, worker_id: &str) -> String {
    template
        .replace("$RUNPOD_POD_ID", worker_id)
        .replace("$ID", worker_id)
}

fn job_get_url(template: &str, count: usize) -> String {
    if count > 1 {
        template.replace("/job-take/", "/job-take-batch/")
    } else {
        template.to_string()
    }
}

fn append_query(url: &mut String, key: &str, value: &str) {
    let separator = if url.contains('?') { '&' } else { '?' };
    url.push(separator);
    url.push_str(key);
    url.push('=');
    url.push_str(value);
}

fn uuidish_worker_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("lumen-runpod-{nanos:x}")
}
