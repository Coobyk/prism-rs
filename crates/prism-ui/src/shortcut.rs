use prism_core::{Instance, Paths};
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const RUN_SHORTCUT: &str = r#"PRISM_RS_DATA="$1"; export PRISM_RS_DATA; shift; exec "$@""#;

pub fn create(paths: &Paths, instance: &Instance) -> io::Result<PathBuf> {
    let desktop = dirs::desktop_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Desktop")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Could not locate your desktop"))?;
    let exe = std::env::current_exe()?;
    let data = std::path::absolute(&paths.root)?;
    write_shortcut(&desktop, &exe, &data, &instance.id, &instance.name)
}

fn write_shortcut(
    desktop: &Path,
    exe: &Path,
    data: &Path,
    id: &str,
    name: &str,
) -> io::Result<PathBuf> {
    let exe = utf8_path(exe)?;
    let data = utf8_path(data)?;
    let name = if name.trim().is_empty() {
        "Minecraft"
    } else {
        name.trim()
    };
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec=/bin/sh -c {} prism-rs {} {} launch {}\nIcon=applications-games\nTerminal=false\n",
        key_value(name),
        exec_argument(RUN_SHORTCUT)?,
        exec_argument(data)?,
        exec_argument(exe)?,
        exec_argument(id)?,
    );

    let stem = file_stem(name);
    fs::create_dir_all(desktop)?;
    let mut temp = tempfile::NamedTempFile::new_in(desktop)?;
    temp.write_all(entry.as_bytes())?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o700))?;
    for number in 1..=u32::MAX {
        let filename = if number == 1 {
            format!("{stem}.desktop")
        } else {
            format!("{stem} ({number}).desktop")
        };
        let path = desktop.join(filename);
        match temp.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => temp = error.file,
            Err(error) => return Err(error.error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not find an unused shortcut name",
    ))
}

fn file_stem(name: &str) -> String {
    let mut stem = String::new();
    for ch in name.trim_start_matches('.').trim_start().chars() {
        if stem.len() + ch.len_utf8() > 200 {
            break;
        }
        stem.push(if ch.is_control() || matches!(ch, '/' | '\\') {
            '_'
        } else {
            ch
        });
    }
    if stem.is_empty() {
        "Minecraft".into()
    } else {
        stem
    }
}

fn utf8_path(path: &Path) -> io::Result<&str> {
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Shortcut paths must be valid UTF-8",
        )
    })
}

fn key_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\0', "")
}

fn exec_argument(argument: &str) -> io::Result<String> {
    if argument.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Shortcut arguments cannot contain control characters",
        ));
    }
    // Exec has its own quoting, applied after desktop-entry value unescaping.
    let mut quoted = String::from("\"");
    for ch in argument.chars() {
        match ch {
            '"' | '`' | '$' | '\\' => quoted.push('\\'),
            '%' => quoted.push('%'),
            _ => {}
        }
        quoted.push(ch);
    }
    quoted.push('"');
    Ok(key_value(&quoted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_exec_arguments_and_escapes_field_codes() {
        assert_eq!(
            exec_argument(r#"/tmp/a $b "quoted" `c` \ %f"#).unwrap(),
            r#""/tmp/a \\$b \\"quoted\\" \\`c\\` \\\\ %%f""#,
        );
        assert!(exec_argument("id\nTerminal=true").is_err());
    }

    #[test]
    fn creates_executable_entries_without_overwriting_colliding_names() {
        let temp = tempfile::tempdir().unwrap();
        let desktop = temp.path();
        let existing = desktop.join("_Saves.desktop");
        fs::write(&existing, "keep me").unwrap();
        let path = write_shortcut(
            desktop,
            Path::new("/opt/Prism RS/prism-rs"),
            Path::new("/tmp/prism data"),
            "world-id",
            "../Saves",
        )
        .unwrap();
        assert_eq!(path, desktop.join("_Saves (2).desktop"));
        assert_eq!(fs::read_to_string(existing).unwrap(), "keep me");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o700,
            0o700
        );
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("Name=../Saves\n"));
        assert!(text.contains(r#" "/tmp/prism data" "/opt/Prism RS/prism-rs" launch "world-id""#));
    }

    #[test]
    fn shortcut_command_accepts_executable_paths_with_equals() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("prism=rs");
        fs::write(
            &exe,
            "#!/bin/sh\nprintf '%s\\n' \"$PRISM_RS_DATA\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", RUN_SHORTCUT, "prism-rs", "/tmp/prism data"])
            .arg(exe)
            .args(["launch", "world id"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"/tmp/prism data\nlaunch\nworld id\n");
    }
}
