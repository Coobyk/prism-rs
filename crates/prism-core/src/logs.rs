use flate2::read::MultiGzDecoder;
use regex::Regex;
use serde::Deserialize;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, SystemTime},
};

const MAX_LOG_BYTES: u64 = 12 * 1024 * 1024;
const PASTE_ENDPOINT: &str = "https://api.mclo.gs/1/log";

static REDACTIONS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?i)([a-z]:\\users\\)[^\\\r\n]+", "${1}********"),
        (r"(?i)([a-z]:/users/|/(?:home|users)/)[^/\s\x22]+", "${1}********"),
        (r"(?i)(\(session id is )[^)\r\n]+", "${1}<SESSION_TOKEN>"),
        (r#"(?i)(new refresh token:\s*")[^"\r\n]+"#, "${1}<TOKEN>"),
        (r#"(?i)("(?:device_code|access_token|refresh_token|identityToken|token|client_secret)"\s*:\s*")[^"\r\n]+"#, "${1}<TOKEN>"),
        (r#"(?i)(--(?:accessToken|clientToken|username)(?:\s+|=))(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,\]]+)"#, "${1}<REDACTED>"),
        (r"(?i)(\bsetting user:\s*)[^\s,\]]+", "${1}<USERNAME>"),
        (r"(?i)(\bauthorization:\s*bearer\s+)[^\s,\]]+", "${1}<TOKEN>"),
    ]
    .into_iter()
    .map(|(pattern, replacement)| (Regex::new(pattern).unwrap(), replacement))
    .collect()
});

pub fn anonymize_log(text: &str) -> String {
    let mut result = std::borrow::Cow::Borrowed(text);
    for (rule, replacement) in &*REDACTIONS {
        if let std::borrow::Cow::Owned(redacted) = rule.replace_all(&result, *replacement) {
            result = redacted.into();
        }
    }
    result.into_owned()
}

#[derive(Deserialize)]
struct PasteResponse {
    success: bool,
    url: Option<String>,
}

/// Uploads a log after anonymizing it. Callers must get consent first.
pub async fn upload_log(text: &str) -> Result<reqwest::Url, String> {
    upload_to(PASTE_ENDPOINT, text).await
}

async fn upload_to(endpoint: &str, text: &str) -> Result<reqwest::Url, String> {
    if text.trim().is_empty() {
        return Err("There is no log to upload.".into());
    }
    if text.len() as u64 > MAX_LOG_BYTES {
        return Err("The log is too large to upload.".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize the upload client.")?;
    let response = client
        .post(endpoint)
        .form(&[("content", anonymize_log(text))])
        .send()
        .await
        .map_err(|_| "Could not reach mclo.gs.")?;
    if !response.status().is_success() {
        return Err(format!(
            "mclo.gs returned HTTP {}.",
            response.status().as_u16()
        ));
    }
    let mut response = response;
    let mut json = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read the upload result.")?
    {
        if json.len() + chunk.len() > 16 * 1024 {
            return Err("mclo.gs returned an oversized response.".into());
        }
        json.extend_from_slice(&chunk);
    }
    let paste: PasteResponse =
        serde_json::from_slice(&json).map_err(|_| "mclo.gs returned an invalid response.")?;
    if !paste.success {
        return Err("mclo.gs rejected the upload.".into());
    }
    paste
        .url
        .and_then(|url| reqwest::Url::parse(&url).ok())
        .filter(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("mclo.gs")
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path().strip_prefix('/').is_some_and(|id| {
                    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
        })
        .ok_or_else(|| "mclo.gs returned an invalid share URL.".into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    created: Option<SystemTime>,
    #[cfg(unix)]
    unix: (u64, u64),
}

impl Identity {
    fn read(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            created: metadata.created().ok(),
            #[cfg(unix)]
            unix: (metadata.dev(), metadata.ino()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    id: Identity,
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    fn read(metadata: &fs::Metadata) -> Self {
        Self {
            id: Identity::read(metadata),
            modified: metadata.modified().ok(),
            len: metadata.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFile {
    root: PathBuf,
    directory: PathBuf,
    directory_id: Identity,
    name: String,
    stamp: Stamp,
}

fn log_directory(root: &Path) -> io::Result<(PathBuf, Identity)> {
    let path = root.join("logs");
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() {
        return Err(io::Error::other(
            "The logs folder is not a regular directory.",
        ));
    }
    Ok((fs::canonicalize(path)?, Identity::read(&meta)))
}

impl LogFile {
    pub fn list(game_root: &Path) -> io::Result<Vec<Self>> {
        let (directory, directory_id) = match log_directory(game_root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            result => result?,
        };
        let mut logs = Vec::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if !(lower.ends_with(".log") || lower.ends_with(".log.gz") || lower.ends_with(".txt")) {
                continue;
            }
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                logs.push(Self {
                    root: game_root.to_owned(),
                    directory: directory.clone(),
                    directory_id: directory_id.clone(),
                    name,
                    stamp: Stamp::read(&metadata),
                });
            }
        }
        logs.sort_by(|left, right| {
            right
                .stamp
                .modified
                .cmp(&left.stamp.modified)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(logs)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn game_root(&self) -> &Path {
        &self.root
    }

    fn checked_path(&self) -> io::Result<PathBuf> {
        let (directory, id) = log_directory(&self.root)?;
        let path = self.directory.join(&self.name);
        if directory == self.directory && id == self.directory_id {
            let meta = fs::symlink_metadata(&path)?;
            if meta.is_file() && Stamp::read(&meta) == self.stamp {
                return Ok(path);
            }
        }
        Err(io::Error::other(
            "The log changed on disk. Refresh and try again.",
        ))
    }

    pub fn read(&self) -> io::Result<String> {
        let file = fs::File::open(self.checked_path()?)?;
        if Stamp::read(&file.metadata()?) != self.stamp {
            return Err(io::Error::other(
                "The log changed on disk. Refresh and try again.",
            ));
        }
        let mut bytes = Vec::new();
        if self.name.to_ascii_lowercase().ends_with(".gz") {
            MultiGzDecoder::new(file)
                .take(MAX_LOG_BYTES + 1)
                .read_to_end(&mut bytes)?;
        } else {
            file.take(MAX_LOG_BYTES + 1).read_to_end(&mut bytes)?;
        }
        if bytes.len() as u64 > MAX_LOG_BYTES {
            return Err(io::Error::other(
                "This log exceeds the 12 MiB viewing limit.",
            ));
        }
        self.checked_path()?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn delete(&self) -> io::Result<()> {
        fs::remove_file(self.checked_path()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn anonymizes_paths_usernames_and_tokens_before_sharing() {
        let text = r#"Paths: C:\Users\Alex\game C:/Users/Casey/game /home/sam/.minecraft /Users/taylor/world
[main/INFO]: Setting user: AppPlayer
(Session ID is token:private-session:uuid)
New Refresh Token: "private-refresh"
{"device_code" :  "private-device", "access_token":"private-access", "refresh_token": "private-json"}
--accessToken private-cli --username AppPlayer
Loaded 42 mods & started +1 job."#;
        let redacted = anonymize_log(text);
        for value in ["Alex", "Casey", "sam", "taylor", "AppPlayer", "private-"] {
            assert!(!redacted.contains(value), "{value} was not redacted");
        }
        assert!(redacted.contains("/home/********/.minecraft"));
        assert!(redacted.contains("Loaded 42 mods & started +1 job."));
        assert_eq!(redacted, anonymize_log(&redacted));
    }

    #[test]
    fn reads_plain_and_gzip_logs_and_rejects_a_replaced_file() {
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join("logs");
        fs::create_dir(&logs).unwrap();
        fs::write(logs.join("latest.log"), "Latest").unwrap();
        fs::write(logs.join("ignored.json"), "Not a log").unwrap();
        let mut gzip = flate2::write::GzEncoder::new(
            fs::File::create(logs.join("older.log.gz")).unwrap(),
            flate2::Compression::default(),
        );
        gzip.write_all(b"Earlier\n").unwrap();
        gzip.finish().unwrap();
        let files = LogFile::list(root.path()).unwrap();
        assert_eq!(files.len(), 2);
        let latest = files
            .iter()
            .find(|file| file.name() == "latest.log")
            .unwrap();
        let older = files
            .iter()
            .find(|file| file.name() == "older.log.gz")
            .unwrap();
        assert_eq!(latest.read().unwrap(), "Latest");
        assert_eq!(older.read().unwrap(), "Earlier\n");
        fs::rename(logs.join("latest.log"), logs.join("moved.log")).unwrap();
        fs::write(logs.join("latest.log"), "New log").unwrap();
        assert!(latest.read().is_err());
        assert!(latest.delete().is_err());
        assert_eq!(
            fs::read_to_string(logs.join("latest.log")).unwrap(),
            "New log"
        );
        older.delete().unwrap();
        assert!(!logs.join("older.log.gz").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_logs_and_log_directories() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join("logs");
        fs::create_dir(&logs).unwrap();
        let outside = root.path().join("outside.log");
        fs::write(&outside, "outside").unwrap();
        symlink(&outside, logs.join("alias.log")).unwrap();
        assert!(LogFile::list(root.path()).unwrap().is_empty());
        fs::rename(&logs, root.path().join("actual")).unwrap();
        symlink("actual", &logs).unwrap();
        assert!(LogFile::list(root.path()).is_err());
    }

    #[tokio::test]
    async fn upload_posts_only_anonymized_form_data() {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/1/log", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = BufReader::new(&mut stream);
            let mut line = String::new();
            request.read_line(&mut line).unwrap();
            assert_eq!(line, "POST /1/log HTTP/1.1\r\n");
            let mut length = None;
            loop {
                line.clear();
                assert_ne!(request.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            request.read_exact(&mut body).unwrap();
            let response = r#"{"success":true,"url":"https://mclo.gs/Proof123"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{response}",
                response.len()
            )
            .unwrap();
            String::from_utf8(body).unwrap()
        });
        let url = upload_to(&endpoint, "Setting user: TestPlayer\nx+y & z")
            .await
            .unwrap();
        assert_eq!(url.as_str(), "https://mclo.gs/Proof123");
        assert_eq!(
            server.join().unwrap(),
            "content=Setting+user%3A+%3CUSERNAME%3E%0Ax%2By+%26+z"
        );
    }
}
