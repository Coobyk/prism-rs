//! HTTP downloading with caching and checksum validation. Mirrors
//! PrismLauncher's `net/` (NetJob / FileSink / HttpMetaCache).

use futures::stream::{self, StreamExt};
use reqwest::Client;
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyMode {
    System,
    Direct,
    Http,
    Socks5,
}

impl ProxyMode {
    pub const ALL: [Self; 4] = [Self::System, Self::Direct, Self::Http, Self::Socks5];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::System => "Default",
            Self::Direct => "None",
            Self::Http => "HTTP",
            Self::Socks5 => "SOCKS5",
        }
    }

    pub fn from_settings(settings: &crate::settings::Settings) -> Self {
        Self::ALL
            .into_iter()
            .find(|mode| settings.get("ProxyType") == Some(mode.config_value()))
            .unwrap_or(Self::System)
    }
}

fn http_builder(settings: &crate::settings::Settings) -> Result<reqwest::ClientBuilder, &'static str> {
    let mode = ProxyMode::from_settings(settings);
    let builder = Client::builder();
    let scheme = match mode {
        ProxyMode::System => return Ok(builder),
        ProxyMode::Direct => return Ok(builder.no_proxy()),
        ProxyMode::Http => "http",
        ProxyMode::Socks5 => "socks5h",
    };
    let host = settings.get("ProxyAddr").or_else(|| settings.get("ProxyHostName")).unwrap_or("127.0.0.1");
    let port = settings
        .get("ProxyPort")
        .unwrap_or("8080")
        .parse::<u16>()
        .ok()
        .filter(|&port| port > 0)
        .ok_or("Enter a proxy port between 1 and 65535.")?;
    let host = host.trim();
    if host.is_empty() || host.contains(['/', '\\', '?', '#', '@', '%']) || host.contains(char::is_whitespace) {
        return Err("Enter a proxy hostname or IP address, without a scheme or port.");
    }
    let host = match host.parse::<std::net::Ipv6Addr>() {
        Ok(ip) => format!("[{ip}]"),
        Err(_) => reqwest::Url::parse(&format!("http://{host}"))
            .ok()
            .filter(|_| !host.contains(':') || (host.starts_with('[') && host.ends_with(']')))
            .and_then(|url| url.host_str().map(str::to_owned))
            .ok_or("Enter a proxy hostname or IP address, without a scheme or port.")?,
    };
    let mut address = reqwest::Url::parse(&format!("{scheme}://{host}:{port}"))
        .map_err(|_| "Invalid proxy address.")?;
    if let Some(user) = settings.get("ProxyUser").filter(|user| !user.is_empty()) {
        address.set_username(user).map_err(|_| "Invalid proxy credentials.")?;
        address.set_password(settings.get("ProxyPass")).map_err(|_| "Invalid proxy credentials.")?;
    }
    let proxy = reqwest::Proxy::all(address).map_err(|_| "Invalid proxy configuration.")?;
    Ok(builder.proxy(proxy))
}

pub(crate) fn configured_http_builder() -> reqwest::ClientBuilder {
    let settings = crate::settings::Settings::load(&crate::Paths::new().global_config());
    http_builder(&settings).unwrap_or_else(|error| {
        tracing::warn!("{error} Check the launcher Proxy settings.");
        Client::builder().proxy(reqwest::Proxy::all("http://127.0.0.1:0").unwrap())
    })
}

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
    client: std::sync::RwLock<Client>,
}

impl Downloader {
    pub fn new() -> Self {
        Self { client: Self::build_client(configured_http_builder()).expect("failed to build http client").into() }
    }

    fn build_client(builder: reqwest::ClientBuilder) -> Result<Client, reqwest::Error> {
        builder
            .user_agent(format!(
                "prism-rs/{} (github.com/Coobyk/prism-rs)",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(120))
            .build()
    }

    pub fn client(&self) -> Client {
        self.client.read().unwrap().clone()
    }

    pub fn update_proxy(
        &self,
        settings: &crate::settings::Settings,
        config_path: &Path,
    ) -> Result<(), String> {
        let client = Self::build_client(http_builder(settings)?)
            .map_err(|_| "Could not configure the launcher proxy.")?;
        settings.save(config_path).map_err(|error| format!("Save failed: {error}"))?;
        *self.client.write().unwrap() = client;
        Ok(())
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
        let parent = req.dest.parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        tokio::fs::create_dir_all(parent).await?;

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

        let response = self.client().get(&req.url).send().await?;
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
        let parent = parent.to_owned();
        let dest = req.dest.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            use std::io::Write;
            let mut builder = tempfile::Builder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                builder.permissions(std::fs::Permissions::from_mode(0o666));
            }
            let mut tmp = builder.tempfile_in(parent)?;
            tmp.write_all(&bytes)?;
            tmp.persist(dest).map_err(|error| error.error)?;
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)??;
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
        let response = self.client().get(url).send().await?;
        if !response.status().is_success() {
            return Err(NetError::Status { status: response.status().as_u16(), url: url.to_string() });
        }
        Ok(response.json().await?)
    }

    /// Fetch text (e.g. RSS).
    pub async fn get_text(&self, url: &str) -> Result<String, NetError> {
        let response = self.client().get(url).send().await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[tokio::test]
    async fn saved_http_proxy_routes_downloader_requests() {
        let dir = tempfile::tempdir().unwrap();
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let mut settings = crate::settings::Settings::default();
        settings.set("ProxyType", "HTTP");
        settings.set("ProxyAddr", "127.0.0.1");
        settings.set("ProxyPort", port.to_string());
        let downloader = Downloader::new();
        let path = dir.path().join("prismlauncher.cfg");
        downloader.update_proxy(&settings, &path).unwrap();
        assert_eq!(ProxyMode::from_settings(&crate::settings::Settings::load(&path)), ProxyMode::Http);
        let server = std::thread::spawn(move || {
            let (stream, _) = socket.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).unwrap();
            assert_eq!(line, "GET http://proxy-test.invalid/check HTTP/1.1\r\n");
            while !line.is_empty() && line != "\r\n" {
                line.clear();
                stream.read_line(&mut line).unwrap();
            }
            stream.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
        });
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), downloader.get_text("http://proxy-test.invalid/check")).await.unwrap().unwrap(), "ok");
        server.join().unwrap();
    }

    #[test]
    fn explicit_proxy_requires_a_host_and_numeric_port() {
        let mut settings = crate::settings::Settings::default();
        settings.set("ProxyType", "SOCKS5");
        settings.set("ProxyPort", "0");
        assert!(http_builder(&settings).is_err());
        settings.set("ProxyPort", "1080");
        for address in ["http://proxy.example", "proxy.example:1080", "user@proxy.example"] {
            settings.set("ProxyAddr", address);
            assert!(http_builder(&settings).is_err());
        }
        settings.set("ProxyAddr", "::1");
        assert!(http_builder(&settings).is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_cache_misses_publish_complete_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("shared.jar");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/shared.jar", listener.local_addr().unwrap());
        let payload = vec![b'x'; 8_000_000];
        let checksum = sha1_hex(&payload);
        let server = std::thread::spawn(move || {
            let mut connections = Vec::new();
            for _ in 0..2 {
                let (connection, _) = listener.accept().unwrap();
                connection.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(connection);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() != 0 && line != "\r\n" {
                    line.clear();
                }
                connections.push(reader.into_inner());
            }
            std::thread::scope(|scope| {
                for mut connection in connections {
                    let payload = &payload;
                    scope.spawn(move || {
                        write!(connection, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", payload.len()).unwrap();
                        connection.write_all(payload).unwrap();
                    });
                }
            });
        });

        let downloader = Downloader { client: Client::builder().no_proxy().build().unwrap().into() };
        let req = DownloadRequest::new(url, &dest);
        let (first, second) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(downloader.download(&req), downloader.download(&req))
        }).await.unwrap();
        server.join().unwrap();
        assert_eq!(first.unwrap(), dest);
        assert_eq!(second.unwrap(), dest);
        Downloader::verify_sha1(&dest, &checksum).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let reference = dir.path().join("ordinary-file");
            std::fs::write(&reference, []).unwrap();
            assert_eq!(
                dest.metadata().unwrap().permissions().mode(),
                reference.metadata().unwrap().permissions().mode()
            );
        }
    }
}
