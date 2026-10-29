//! HTTP downloading with caching and checksum validation. Mirrors
//! PrismLauncher's `net/` (NetJob / FileSink / HttpMetaCache).

use futures::stream::{self, StreamExt};
use reqwest::Client;
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    Checksum { url: String, expected: String, actual: String },
    #[error("bad status {status} for {url}")]
    Status { status: u16, url: String },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// One file to download, with an optional expected SHA-1.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub dest: PathBuf,
    pub sha1: Option<String>,
    /// Skip the download when the destination already exists (even without
    /// checksum verification).
    pub skip_if_exists: bool,
}

impl DownloadRequest {
    pub fn new(url: impl Into<String>, dest: impl Into<PathBuf>) -> Self {
        DownloadRequest {
            url: url.into(),
            dest: dest.into(),
            sha1: None,
            skip_if_exists: true,
        }
    }

    pub fn with_sha1(mut self, sha1: impl Into<String>) -> Self {
        self.sha1 = Some(sha1.into());
        self
    }
}

/// Progress snapshot for a multi-file download job.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub finished: usize,
    pub total: usize,
    pub current_url: Option<String>,
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub struct Downloader {
    client: Client,
}

impl Downloader {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(format!(
                "prism-rs/{} (github.com/Coobyk/prism-rs)",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build http client");
        Downloader { client }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Validate an existing file against an expected SHA-1.
    pub fn verify_sha1(path: &Path, expected: &str) -> Result<(), NetError> {
        let bytes = std::fs::read(path)?;
        let actual = sha1_hex(&bytes);
        if actual != expected.to_lowercase() {
            return Err(NetError::Checksum {
                url: path.display().to_string(),
                expected: expected.to_lowercase(),
                actual,
            });
        }
        Ok(())
    }

    /// Download a single file. When the destination exists and matches the
    /// checksum (or no checksum is given), nothing is downloaded.
    pub async fn download(&self, req: &DownloadRequest) -> Result<PathBuf, NetError> {
        if let Some(parent) = req.dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let existing_valid = match tokio::fs::read(&req.dest).await {
            Ok(bytes) => match &req.sha1 {
                Some(sha1) => !bytes.is_empty() && sha1_hex(&bytes) == sha1.to_lowercase(),
                None => req.skip_if_exists && !bytes.is_empty(),
            },
            Err(_) => false,
        };
        if existing_valid {
            return Ok(req.dest.clone());
        }

        let response = self.client.get(&req.url).send().await?;
        if !response.status().is_success() {
            return Err(NetError::Status { status: response.status().as_u16(), url: req.url.clone() });
        }
        let bytes = response.bytes().await?;
        if let Some(sha1) = &req.sha1 {
            let actual = sha1_hex(&bytes);
            if actual != sha1.to_lowercase() {
                return Err(NetError::Checksum {
                    url: req.url.clone(),
                    expected: sha1.to_lowercase(),
                    actual,
                });
            }
        }
        // Atomic-ish write: write to tmp then rename.
        let tmp = req.dest.with_extension("download");
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, &req.dest).await?;
        Ok(req.dest.clone())
    }

    /// Download many files with bounded concurrency, reporting progress
    /// through the provided channel-like closure.
    pub async fn download_all(
        self: &Arc<Self>,
        requests: Vec<DownloadRequest>,
        concurrency: usize,
        progress: impl Fn(DownloadProgress) + Send + Sync + 'static,
    ) -> Result<(), NetError> {
        let total = requests.len();
        let finished = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let progress = Arc::new(progress);
        let errors: Vec<NetError> = stream::iter(requests)
            .map(|req| {
                let this = self.clone();
                let finished = finished.clone();
                let progress = progress.clone();
                async move {
                    let result = this.download(&req).await;
                    let done = finished.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    progress(DownloadProgress {
                        finished: done,
                        total,
                        current_url: Some(req.url.clone()),
                    });
                    result
                }
            })
            .buffer_unordered(concurrency.max(1))
            .filter_map(|r| async move { r.err() })
            .collect()
            .await;
        match errors.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Fetch a URL and parse it as JSON.
    pub async fn get_json(&self, url: &str) -> Result<serde_json::Value, NetError> {
        let response = self.client.get(url).send().await?;
        if !response.status().is_success() {
            return Err(NetError::Status { status: response.status().as_u16(), url: url.to_string() });
        }
        Ok(response.json().await?)
    }

    /// Fetch text (e.g. RSS).
    pub async fn get_text(&self, url: &str) -> Result<String, NetError> {
        let response = self.client.get(url).send().await?;
        if !response.status().is_success() {
            return Err(NetError::Status { status: response.status().as_u16(), url: url.to_string() });
        }
        Ok(response.text().await?)
    }
}

/// Free-standing JSON fetch for callers that only have a client.
pub async fn client_json(client: &reqwest::Client, url: &str) -> Result<serde_json::Value, NetError> {
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(NetError::Status { status: response.status().as_u16(), url: url.to_string() });
    }
    Ok(response.json().await?)
}

impl Default for Downloader {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared tokio runtime for prism-core async work; the UI spawns jobs onto
/// this and receives results via channels.
pub struct CoreRuntime {
    runtime: tokio::runtime::Runtime,
    pub downloader: Arc<Downloader>,
}

impl CoreRuntime {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        CoreRuntime {
            runtime,
            downloader: Arc::new(Downloader::new()),
        }
    }

    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: futures::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(future)
    }

    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }
}

impl Default for CoreRuntime {
    fn default() -> Self {
        Self::new()
    }
}
