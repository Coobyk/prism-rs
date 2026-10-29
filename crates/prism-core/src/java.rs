//! Java detection and version checking. Mirrors `launcher/java/JavaUtils`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Find a usable `java` executable: JAVA_HOME, PATH, and common install
/// locations.
pub fn find_java() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let candidate = Path::new(&home).join("bin").join(java_binary_name());
        if candidate.exists() {
            return Some(candidate);
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(java_binary_name());
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for dir in common_java_dirs() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("bin").join(java_binary_name());
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    None
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
}
