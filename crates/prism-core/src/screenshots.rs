use crate::worlds::{plain_name, FileStamp};
use reqwest::{multipart, Url};
use serde::Deserialize;
use std::{
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub const IMGUR_CLIENT_ID_ENV: &str = "PRISM_RS_IMGUR_CLIENT_ID";
const IMGUR_BASE_URL: &str = "https://api.imgur.com/3/";
const MAX_PNG_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Directory {
    path: PathBuf,
    created: Option<SystemTime>,
    #[cfg(unix)]
    id: (u64, u64),
}

impl Directory {
    fn resolve(game_root: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(game_root.join("screenshots"))?;
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_dir() {
            return Err(io::Error::other("Screenshots is not a directory."));
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            path,
            created: meta.created().ok(),
            #[cfg(unix)]
            id: (meta.dev(), meta.ino()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    game_root: PathBuf,
    directory: Directory,
    name: String,
    stamp: FileStamp,
}

fn is_png_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

fn regular_stamp(path: &Path) -> io::Result<FileStamp> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(io::Error::other("Not a regular screenshot file."));
    }
    Ok((&meta).into())
}

impl Screenshot {
    pub fn list(game_root: &Path) -> io::Result<Vec<Self>> {
        let directory = match Directory::resolve(game_root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            result => result?,
        };
        let mut screenshots = Vec::new();
        for entry in fs::read_dir(&directory.path)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !is_png_name(&name) || !entry.file_type()?.is_file() {
                continue;
            }
            match regular_stamp(&entry.path()) {
                Ok(stamp) => screenshots.push(Self {
                    game_root: game_root.to_owned(),
                    directory: directory.clone(),
                    name,
                    stamp,
                }),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
        }
        screenshots.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(screenshots)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn game_root(&self) -> &Path {
        &self.game_root
    }

    fn checked_path(&self) -> io::Result<PathBuf> {
        let path = self.directory.path.join(&self.name);
        if Directory::resolve(&self.game_root)? == self.directory
            && regular_stamp(&path)? == self.stamp
        {
            return Ok(path);
        }
        Err(io::Error::other(
            "The screenshot changed. Refresh the list and try again.",
        ))
    }

    pub fn read_png(&self) -> io::Result<Vec<u8>> {
        let file = fs::File::open(self.checked_path()?)?;
        if FileStamp::from(&file.metadata()?) != self.stamp {
            return Err(io::Error::other(
                "The screenshot changed. Refresh the list and try again.",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_PNG_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_PNG_BYTES {
            return Err(io::Error::other(
                "This screenshot exceeds the 20 MiB limit.",
            ));
        }
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "This file is not a PNG.",
            ));
        }
        self.checked_path()?;
        Ok(bytes)
    }

    pub fn rename(&self, name: &str) -> io::Result<PathBuf> {
        self.rename_with_link(name, |source, target| fs::hard_link(source, target))
    }

    fn rename_with_link(
        &self,
        name: &str,
        link: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> io::Result<PathBuf> {
        let name = name.trim();
        if !plain_name(name)
            || name.ends_with('.')
            || name.starts_with('.')
            || name
                .chars()
                .any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Enter a valid file name.",
            ));
        }
        let name = if is_png_name(name) {
            name.to_owned()
        } else {
            format!("{name}.png")
        };
        let display_path = self.game_root.join("screenshots").join(&name);
        let path = self.checked_path()?;
        if name == self.name {
            return Ok(display_path);
        }
        let target = self.directory.path.join(name);
        match link(&path, &target) {
            Ok(()) => {
                if let Err(error) = self.checked_path().and_then(|path| fs::remove_file(path)) {
                    if regular_stamp(&target).is_ok_and(|stamp| stamp == self.stamp) {
                        let _ = fs::remove_file(target);
                    }
                    return Err(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Err(error),
            Err(_) => {
                let path = self.checked_path()?;
                match fs::symlink_metadata(&target) {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "A file already exists with that name.",
                        ))
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                fs::rename(path, &target)?;
            }
        }
        Ok(display_path)
    }

    pub fn delete(&self) -> io::Result<()> {
        fs::remove_file(self.checked_path()?)
    }
}

pub fn imgur_client_id() -> Option<String> {
    std::env::var(IMGUR_CLIENT_ID_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[derive(Deserialize)]
struct ImgurResponse {
    success: bool,
    data: Option<ImgurImage>,
}

#[derive(Deserialize)]
struct ImgurImage {
    link: String,
}

/// Sends a screenshot to Imgur. The caller must get consent first.
pub async fn upload_to_imgur(client_id: &str, name: &str, bytes: Vec<u8>) -> Result<Url, String> {
    upload_imgur(IMGUR_BASE_URL, client_id, name, bytes).await
}

async fn upload_imgur(
    base: &str,
    client_id: &str,
    name: &str,
    bytes: Vec<u8>,
) -> Result<Url, String> {
    if client_id.is_empty() || !client_id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(format!(
            "Set {IMGUR_CLIENT_ID_ENV} to a valid Imgur client ID."
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize the Imgur client.")?;
    let image = multipart::Part::bytes(bytes)
        .file_name("screenshot.png")
        .mime_str("image/png")
        .map_err(|_| "Invalid screenshot media type.")?;
    let title = Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Screenshot");
    let form = multipart::Form::new()
        .part("image", image)
        .text("type", "file")
        .text("title", title.to_owned());
    let mut response = client
        .post(format!("{base}image"))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Client-ID {client_id}"),
        )
        .multipart(form)
        .send()
        .await
        .map_err(|_| "Could not reach Imgur.")?;
    if !response.status().is_success() {
        return Err(format!(
            "Imgur returned HTTP {}.",
            response.status().as_u16()
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read the Imgur response.")?
    {
        if body.len() + chunk.len() > 16 * 1024 {
            return Err("Imgur returned an oversized response.".into());
        }
        body.extend_from_slice(&chunk);
    }
    let data: ImgurResponse =
        serde_json::from_slice(&body).map_err(|_| "Imgur returned an invalid response.")?;
    if !data.success {
        return Err("Imgur rejected the screenshot.".into());
    }
    data.data
        .and_then(|image| Url::parse(&image.link).ok())
        .filter(|url| {
            url.scheme() == "https"
                && matches!(url.host_str(), Some("i.imgur.com" | "imgur.com"))
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
        })
        .ok_or_else(|| "Imgur returned an invalid share URL.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn renames_without_overwriting_and_rejects_replaced_targets() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("screenshots");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("original.png"), b"original").unwrap();
        fs::write(dir.join("taken.png"), b"taken").unwrap();
        let shots = Screenshot::list(root.path()).unwrap();
        assert_eq!(shots.len(), 2);
        let original = &shots[0];
        assert!(original.rename("taken").is_err());
        assert_eq!(fs::read(dir.join("taken.png")).unwrap(), b"taken");
        assert!(original.rename("../unsafe").is_err());
        assert_eq!(original.rename("renamed").unwrap(), dir.join("renamed.png"));
        let renamed = &Screenshot::list(root.path()).unwrap()[0];
        fs::rename(dir.join("renamed.png"), dir.join("moved.png")).unwrap();
        fs::write(dir.join("renamed.png"), b"changed").unwrap();
        assert!(renamed.rename("elsewhere").is_err());
        assert!(renamed.delete().is_err());
        assert_eq!(fs::read(dir.join("renamed.png")).unwrap(), b"changed");
        Screenshot::list(root.path()).unwrap()[1].delete().unwrap();
        assert!(!dir.join("renamed.png").exists());
    }

    #[test]
    fn rename_falls_back_without_hard_links() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("screenshots");
        fs::create_dir(&dir).unwrap();
        let source = dir.join("original.png");
        let target = dir.join("renamed.png");
        fs::write(&source, b"original").unwrap();
        fs::write(&target, b"taken").unwrap();
        let shots = Screenshot::list(root.path()).unwrap();
        let screenshot = &shots[0];
        let unavailable = |_: &Path, _: &Path| Err(io::ErrorKind::PermissionDenied.into());

        assert_eq!(
            screenshot
                .rename_with_link("renamed", unavailable)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&target).unwrap(), b"taken");
        fs::remove_file(&target).unwrap();
        assert_eq!(
            screenshot
                .rename_with_link("renamed", |_, _| Err(io::ErrorKind::AlreadyExists.into()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            screenshot.rename_with_link("renamed", unavailable).unwrap(),
            target
        );
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(!source.exists());

        let renamed = &Screenshot::list(root.path()).unwrap()[0];
        assert!(renamed
            .rename_with_link("elsewhere", |path, _| {
                fs::rename(path, &source).unwrap();
                fs::write(path, b"replacement").unwrap();
                Err(io::ErrorKind::Unsupported.into())
            })
            .is_err());
        assert_eq!(fs::read(&target).unwrap(), b"replacement");
        assert!(!dir.join("elsewhere.png").exists());
    }

    #[cfg(unix)]
    #[test]
    fn follows_linked_directory_but_never_acts_on_linked_files() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("real");
        fs::create_dir(&dir).unwrap();
        fs::create_dir(root.path().join("next")).unwrap();
        let list_path = root.path().join("screenshots");
        symlink("real", &list_path).unwrap();
        fs::write(dir.join("real.PNG"), b"\x89PNG\r\n\x1a\nfixture").unwrap();
        symlink("real.PNG", dir.join("link.png")).unwrap();
        fs::write(dir.join("not.png"), b"wrong format").unwrap();
        fs::create_dir(dir.join("folder.png")).unwrap();
        let entries = Screenshot::list(root.path()).unwrap();
        assert_eq!(
            entries.iter().map(Screenshot::name).collect::<Vec<_>>(),
            ["not.png", "real.PNG"]
        );
        assert!(entries[0].read_png().is_err());
        assert_eq!(entries[1].read_png().unwrap(), b"\x89PNG\r\n\x1a\nfixture");
        fs::remove_file(&list_path).unwrap();
        symlink("next", &list_path).unwrap();
        assert!(entries[1].read_png().is_err());
        assert!(entries[1].delete().is_err());
        assert!(dir.join("real.PNG").is_file());
    }

    #[tokio::test]
    async fn sends_imgur_multipart_and_returns_the_image_link() {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/3/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut headers = String::new();
            loop {
                let start = headers.len();
                assert_ne!(reader.read_line(&mut headers).unwrap(), 0);
                if &headers[start..] == "\r\n" {
                    break;
                }
            }
            let lower = headers.to_ascii_lowercase();
            let length = lower
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let response = r#"{"success":true,"data":{"link":"https://i.imgur.com/Example.png"}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{response}",
                response.len()
            )
            .unwrap();
            (headers, String::from_utf8_lossy(&body).into_owned())
        });
        let url = upload_imgur(
            &base,
            "fixture",
            "Test image.png",
            b"\x89PNG\r\n\x1a\nbytes".to_vec(),
        )
        .await
        .unwrap();
        assert_eq!(url.as_str(), "https://i.imgur.com/Example.png");
        let (headers, body) = server.join().unwrap();
        assert!(headers.starts_with("POST /3/image HTTP/1.1\r\n"));
        assert!(headers.contains("authorization: Client-ID fixture\r\n"));
        assert!(headers.contains("content-type: multipart/form-data; boundary="));
        assert!(body.contains("name=\"image\"; filename=\"screenshot.png\""));
        assert!(body.contains("name=\"title\"\r\n\r\nTest image"));
        assert!(body.contains("PNG\r\n\x1a\nbytes"));
    }
}
