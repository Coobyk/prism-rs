//! Java detection and version checking. Mirrors `launcher/java/JavaUtils`.

use crate::net::{DownloadRequest, Downloader, NetError};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// Find a usable `java` executable: JAVA_HOME, PATH, and common install
/// locations.
pub fn find_java() -> Option<PathBuf> {
    java_candidates().into_iter().next()
}

/// Every `java` executable found in JAVA_HOME, PATH and the common install
/// locations, in that order.
fn java_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let candidate = Path::new(&home).join("bin").join(java_binary_name());
        if candidate.exists() {
            candidates.push(candidate);
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(java_binary_name());
            if candidate.is_file() {
                candidates.push(candidate);
            }
        }
    }
    candidates.extend(common_java_dirs().iter().flat_map(|dir| runtimes_in(dir)));
    candidates
}

/// `java` executables of the runtimes directly under `dir`.
fn runtimes_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut runtimes: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().map_or(true, |ext| ext != "partial"))
        .filter_map(|path| runtime_java_binary(&path))
        .collect();
    runtimes.sort();
    runtimes
}

/// The `java` executable of a runtime directory (a JDK/JRE home, or a macOS
/// `jre.bundle` as shipped by Mojang).
pub fn runtime_java_binary(runtime_dir: &Path) -> Option<PathBuf> {
    [runtime_dir.join("bin"), runtime_dir.join("jre.bundle/Contents/Home/bin")]
        .into_iter()
        .map(|bin| bin.join(java_binary_name()))
        .find(|java| java.is_file())
}

/// An installed Java and its version.
#[derive(Debug, Clone, PartialEq)]
pub struct JavaInstall {
    pub path: PathBuf,
    pub version: JavaVersion,
}

/// Every Java found under `extra_dirs` (each holding runtime directories)
/// and in the system locations `find_java` searches, with its version.
/// Mirrors `JavaInstallList`.
pub fn detect_installs(extra_dirs: &[PathBuf]) -> Vec<JavaInstall> {
    let mut seen = std::collections::HashSet::new();
    extra_dirs
        .iter()
        .flat_map(|dir| runtimes_in(dir))
        .chain(java_candidates())
        .filter(|path| seen.insert(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())))
        .filter_map(|path| Some(JavaInstall { version: check_java(&path)?, path }))
        .collect()
}

/// The first install whose major version is one of `majors`, preferring
/// earlier majors.
pub fn compatible_install<'a>(installs: &'a [JavaInstall], majors: &[u32]) -> Option<&'a JavaInstall> {
    majors.iter().find_map(|major| installs.iter().find(|java| java.version.major == *major))
}

fn java_binary_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn common_java_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".jdks"));
        dirs.push(home.join(".local/share/prismlauncher-rs/java"));
        #[cfg(target_os = "macos")]
        dirs.push(home.join("Library/Java/JavaVirtualMachines"));
    }
    #[cfg(target_os = "linux")]
    dirs.push(PathBuf::from("/usr/lib/jvm"));
    dirs
}

/// Parsed `java -version` output.
#[derive(Debug, Clone, PartialEq)]
pub struct JavaVersion {
    pub major: u32,
    pub raw: String,
}

/// Run `java -version` and parse the major version (e.g. 8, 17, 21).
pub fn check_java(java_path: &Path) -> Option<JavaVersion> {
    let output = Command::new(java_path).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr).to_string();
    let text = if text.is_empty() { String::from_utf8_lossy(&output.stdout).to_string() } else { text };
    parse_java_version(&text)
}

/// Parse the major version out of `openjdk version "17.0.2"` style output.
pub fn parse_java_version(text: &str) -> Option<JavaVersion> {
    for line in text.lines() {
        let Some(quote_start) = line.find('"') else { continue };
        let Some(rest) = line.get(quote_start + 1..) else { continue };
        let Some(quote_end) = rest.find('"') else { continue };
        let version = &rest[..quote_end];
        let major: u32 = if let Some(rest) = version.strip_prefix("1.") {
            rest.split('.').next()?.parse().ok()?
        } else {
            version.split('.').next()?.parse().ok()?
        };
        return Some(JavaVersion { major, raw: version.to_string() });
    }
    None
}

/// Mojang's index of the Java runtimes the official launcher installs.
const MOJANG_RUNTIMES_URL: &str =
    "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

#[derive(Debug, thiserror::Error)]
pub enum JavaDownloadError {
    #[error(transparent)]
    Net(#[from] NetError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Mojang publishes no Java runtimes for {os}-{arch}", os = std::env::consts::OS, arch = std::env::consts::ARCH)]
    UnsupportedPlatform,
    #[error("Mojang publishes no {component} Java runtime for {os}")]
    Unavailable { component: String, os: &'static str },
    #[error("malformed Java runtime manifest: {0}")]
    BadManifest(String),
}

/// Mojang's name for this platform in the runtime index.
fn mojang_runtime_os() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux",
        ("linux", "x86") => "linux-i386",
        ("macos", "x86_64") => "mac-os",
        ("macos", "aarch64") => "mac-os-arm64",
        ("windows", "x86_64") => "windows-x64",
        ("windows", "x86") => "windows-x86",
        ("windows", "aarch64") => "windows-arm64",
        _ => return None,
    })
}

/// Download Mojang's `component` runtime (e.g. `java-runtime-delta`) into
/// `java_dir/<component>` and return its `java` executable. Mirrors
/// `AutoInstallJava` + `java/download/ManifestDownloadTask`.
pub async fn install_mojang_runtime(
    downloader: &Arc<Downloader>,
    java_dir: &Path,
    component: &str,
) -> Result<PathBuf, JavaDownloadError> {
    let os = mojang_runtime_os().ok_or(JavaDownloadError::UnsupportedPlatform)?;
    // One install at a time: two launches needing the same runtime would
    // otherwise share a staging directory. The second one reuses the first's.
    static INSTALL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = INSTALL_LOCK.lock().await;
    let runtime_dir = java_dir.join(component);
    if let Some(java) = runtime_java_binary(&runtime_dir) {
        return Ok(java);
    }
    let index = downloader.get_json(MOJANG_RUNTIMES_URL).await?;
    let manifest_url = index[os][component][0]["manifest"]["url"]
        .as_str()
        .ok_or_else(|| JavaDownloadError::Unavailable { component: component.to_string(), os })?;
    let manifest = downloader.get_json(manifest_url).await?;

    // Install into a staging directory so an interrupted download never
    // looks like a usable runtime.
    let staging = java_dir.join(format!("{component}.partial"));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    if let Err(e) = populate_runtime(downloader, &staging, &manifest).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }
    let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
    tokio::fs::rename(&staging, &runtime_dir).await?;
    runtime_java_binary(&runtime_dir)
        .ok_or_else(|| JavaDownloadError::BadManifest(format!("{component} has no {}", java_binary_name())))
}

async fn populate_runtime(
    downloader: &Arc<Downloader>,
    dir: &Path,
    manifest: &serde_json::Value,
) -> Result<(), JavaDownloadError> {
    let bad = |what: &str| JavaDownloadError::BadManifest(what.to_string());
    let files = manifest["files"].as_object().ok_or_else(|| bad("no files"))?;
    let mut downloads = Vec::new();
    let mut executables = Vec::new();
    let mut links = Vec::new();
    for (name, entry) in files {
        let relative = Path::new(name);
        if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(bad(&format!("unsafe path {name:?}")));
        }
        let path = dir.join(relative);
        match entry["type"].as_str() {
            Some("directory") => tokio::fs::create_dir_all(&path).await?,
            Some("file") => {
                let raw = &entry["downloads"]["raw"];
                let url = raw["url"].as_str().ok_or_else(|| bad(&format!("no url for {name}")))?;
                let mut request = DownloadRequest::new(url, &path);
                if let Some(sha1) = raw["sha1"].as_str() {
                    request = request.with_sha1(sha1);
                }
                downloads.push(request);
                if entry["executable"].as_bool() == Some(true) {
                    executables.push(path);
                }
            }
            Some("link") => {
                let target = entry["target"].as_str().ok_or_else(|| bad(&format!("no target for {name}")))?;
                links.push((path, PathBuf::from(target)));
            }
            _ => {}
        }
    }
    downloader.download_all(downloads, 16, |_| {}).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in executables {
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).await?;
        }
        for (path, target) in links {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::symlink(&target, &path).await?;
        }
    }
    #[cfg(not(unix))]
    let _ = (executables, links);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        let text = "openjdk version \"21.0.3\" 2024-04-16\nOpenJDK Runtime Environment";
        let v = parse_java_version(text).unwrap();
        assert_eq!(v.major, 21);
        let text8 = "java version \"1.8.0_392\"\nJava(TM) SE Runtime Environment";
        let v8 = parse_java_version(text8).unwrap();
        assert_eq!(v8.major, 8);
    }

    #[test]
    fn compatible_install_prefers_earlier_majors() {
        let java = |major: u32| JavaInstall {
            path: PathBuf::from(format!("/jdk{major}/bin/java")),
            version: JavaVersion { major, raw: major.to_string() },
        };
        let installs = [java(21), java(17), java(16)];
        assert_eq!(compatible_install(&installs, &[16, 17]), Some(&installs[2]));
        assert_eq!(compatible_install(&installs, &[17]), Some(&installs[1]));
        assert_eq!(compatible_install(&installs, &[8]), None);
    }
}
