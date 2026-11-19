//! Custom launcher icons, keyed by file stem.
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

pub const EXTENSIONS: &[&str] = &["png", "svg", "jpg", "jpeg", "webp", "ico"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub key: String,
    pub path: PathBuf,
}

fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e))
}

pub fn scan(icons_dir: &Path) -> io::Result<Vec<Icon>> {
    let entries = match fs::read_dir(icons_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut icons = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.is_file() && supported(&path) {
            if let Some(key) = path.file_stem().and_then(|s| s.to_str()) {
                icons.push(Icon {
                    key: key.into(),
                    path,
                });
            }
        }
    }
    icons.sort_by(|a, b| (&a.key, &a.path).cmp(&(&b.key, &b.path)));
    Ok(icons)
}

pub fn add_icon(icons_dir: &Path, src_path: &Path) -> io::Result<String> {
    if !supported(src_path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unsupported icon extension (use png, svg, jpg, jpeg, webp or ico)",
        ));
    }
    let stem: String = src_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = if stem.is_empty() { "icon" } else { &stem };
    fs::create_dir_all(icons_dir)?;
    let mut source = fs::File::open(src_path)?;
    let mut suffix = 0;
    loop {
        let key = if suffix == 0 {
            stem.to_string()
        } else {
            format!("{stem}-{suffix}")
        };
        suffix += 1;
        if key == "default" || scan(icons_dir)?.iter().any(|icon| icon.key == key) {
            continue;
        }
        let path = icons_dir.join(format!(
            "{key}.{}",
            src_path.extension().unwrap().to_string_lossy()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut dest) => {
                if let Err(e) = io::copy(&mut source, &mut dest) {
                    let _ = fs::remove_file(path);
                    return Err(e);
                }
                return Ok(key);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

pub fn remove_icon(icons_dir: &Path, key: &str) -> io::Result<()> {
    for icon in scan(icons_dir)?.into_iter().filter(|icon| icon.key == key) {
        fs::remove_file(icon.path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_scan_add_collision_remove() {
        let root = std::env::temp_dir().join(format!("prism-icons-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let dir = root.join("icons");
        assert!(scan(&dir).unwrap().is_empty());
        let source = root.join("red icon.png");
        fs::write(&source, b"red").unwrap();
        assert_eq!(add_icon(&dir, &source).unwrap(), "red_icon");
        assert_eq!(add_icon(&dir, &source).unwrap(), "red_icon-1");
        fs::write(dir.join("red_icon.svg"), b"svg").unwrap();
        fs::write(dir.join("ignored.txt"), b"ignored").unwrap();
        let icons = scan(&dir).unwrap();
        assert_eq!(
            icons.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(),
            ["red_icon", "red_icon", "red_icon-1"]
        );
        assert_eq!(fs::read(dir.join("red_icon.png")).unwrap(), b"red");
        remove_icon(&dir, "red_icon").unwrap();
        assert_eq!(scan(&dir).unwrap().len(), 1);
        remove_icon(&dir, "missing").unwrap();
        assert_eq!(
            add_icon(&dir, &root.join("bad.exe")).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(add_icon(&dir, &root.join("missing.png")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
