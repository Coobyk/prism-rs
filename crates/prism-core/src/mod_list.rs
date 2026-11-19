use crate::modplatform::{updates::mod_list_metadata, ResourceType};
use std::{path::Path, sync::LazyLock};

#[derive(Debug)]
pub struct ModListEntry {
    pub name: String,
    pub version: Option<String>,
    pub authors: Vec<String>,
    pub url: Option<String>,
    pub filename: String,
    pub mod_id: Option<String>,
}

pub fn collect(mods: &Path) -> Vec<ModListEntry> {
    let index = mod_list_metadata(mods);
    crate::resources::scan_folder(mods, ResourceType::Mod)
        .into_iter()
        .map(|resource| {
            let key = resource
                .file_name
                .strip_suffix(".disabled")
                .unwrap_or(&resource.file_name);
            let indexed = index.get(key);
            let fallback_name = || {
                indexed.map_or_else(
                    || {
                        Path::new(key)
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .unwrap_or(key)
                            .to_owned()
                    },
                    |entry| entry.name.clone(),
                )
            };
            ModListEntry {
                name: resource.title.unwrap_or_else(fallback_name),
                version: resource.version,
                authors: resource.authors,
                url: indexed
                    .and_then(|entry| entry.homepage.clone())
                    .or_else(|| {
                        resource
                            .homepage
                            .as_deref()
                            .and_then(web_url)
                            .map(|url| url.to_string())
                    }),
                mod_id: resource.mod_id.or_else(|| Some(fallback_name())),
                filename: resource.file_name,
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Html,
    Markdown,
    Text,
    Json,
    Csv,
    Custom,
}

impl Format {
    pub const ALL: [Self; 6] = [
        Self::Html,
        Self::Markdown,
        Self::Text,
        Self::Json,
        Self::Csv,
        Self::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Html => "HTML",
            Self::Markdown => "Markdown",
            Self::Text => "Plain text",
            Self::Json => "JSON",
            Self::Csv => "CSV",
            Self::Custom => "Custom",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "md",
            Self::Text | Self::Custom => "txt",
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Fields {
    pub version: bool,
    pub authors: bool,
    pub url: bool,
    pub filename: bool,
}

pub fn render(entries: &[ModListEntry], format: Format, fields: Fields, template: &str) -> String {
    match format {
        Format::Json => {
            let rows: Vec<_> = entries.iter().map(|entry| json(entry, fields)).collect();
            serde_json::to_string_pretty(&rows).expect("mod list strings serialize") + "\n"
        }
        Format::Html => {
            let lines: Vec<_> = entries
                .iter()
                .map(|entry| format!("\t<li>{}</li>", descriptive(entry, fields, format)))
                .collect();
            format!(
                "<html><head><meta charset=\"utf-8\"></head><body><ul>\n{}\n</ul></body></html>",
                lines.join("\n")
            )
        }
        _ => entries
            .iter()
            .map(|entry| match format {
                Format::Markdown => format!("- {}", descriptive(entry, fields, format)),
                Format::Text => descriptive(entry, fields, format),
                Format::Csv => csv(entry, fields),
                Format::Custom => custom(entry, template),
                Format::Html | Format::Json => unreachable!(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn web_url(value: &str) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn markdown_escape(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if "\\`*_{}[]<>()#+-.!|~&".contains(ch) {
            escaped.push('\\');
        }
        if matches!(ch, '\r' | '\n') {
            escaped.push(' ');
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

fn descriptive(entry: &ModListEntry, fields: Fields, format: Format) -> String {
    let escape = match format {
        Format::Html => html_escape,
        Format::Markdown => markdown_escape,
        _ => str::to_owned,
    };
    let mut line = escape(&entry.name);
    if let Some(url) = entry
        .url
        .as_deref()
        .filter(|_| fields.url)
        .and_then(web_url)
    {
        line = match format {
            Format::Html => format!("<a href=\"{}\">{line}</a>", html_escape(url.as_str())),
            Format::Markdown => format!(
                "[{line}]({})",
                url.as_str().replace('(', "%28").replace(')', "%29")
            ),
            _ => format!("{line} ({url})"),
        };
    }
    if let Some(version) = entry.version.as_deref().filter(|_| fields.version) {
        line.push_str(&format!(" [{}]", escape(version)));
    }
    if fields.authors && !entry.authors.is_empty() {
        line.push_str(&format!(" by {}", escape(&entry.authors.join(", "))));
    }
    if fields.filename {
        line.push_str(&format!(" ({})", escape(&entry.filename)));
    }
    line
}

fn json(entry: &ModListEntry, fields: Fields) -> serde_json::Value {
    let mut row = serde_json::json!({ "name": entry.name });
    if let Some(url) = entry.url.as_deref().filter(|_| fields.url) {
        row["url"] = url.into();
    }
    if let Some(version) = entry.version.as_deref().filter(|_| fields.version) {
        row["version"] = version.into();
    }
    if fields.authors && !entry.authors.is_empty() {
        row["authors"] = serde_json::json!(entry.authors);
    }
    if fields.filename {
        row["filename"] = entry.filename.as_str().into();
    }
    row
}

fn csv(entry: &ModListEntry, fields: Fields) -> String {
    let authors = entry.authors.join(",");
    let columns = [
        Some(entry.name.as_str()),
        fields.url.then(|| entry.url.as_deref().unwrap_or_default()),
        fields
            .version
            .then(|| entry.version.as_deref().unwrap_or_default()),
        fields.authors.then_some(authors.as_str()),
        fields.filename.then_some(entry.filename.as_str()),
    ];
    columns
        .into_iter()
        .flatten()
        .map(|value| {
            let value = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                std::borrow::Cow::Owned(format!("'{value}"))
            } else {
                std::borrow::Cow::Borrowed(value)
            };
            if value.contains([',', '"', '\r', '\n']) {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                value.into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn custom(entry: &ModListEntry, template: &str) -> String {
    static TOKENS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\{(?:name|mod_id|url|version|authors|filename)\}").unwrap()
    });
    let authors = entry.authors.join(", ");
    TOKENS
        .replace_all(template, |captures: &regex::Captures<'_>| {
            match &captures[0] {
                "{name}" => entry.name.as_str(),
                "{mod_id}" => entry.mod_id.as_deref().unwrap_or_default(),
                "{url}" => entry.url.as_deref().unwrap_or_default(),
                "{version}" => entry.version.as_deref().unwrap_or_default(),
                "{authors}" => authors.as_str(),
                "{filename}" => entry.filename.as_str(),
                _ => unreachable!(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn formats_escape_data_and_toggle_the_optional_fields() {
        let entries = [ModListEntry {
            name: "A, \"<mod>\" {version}".into(),
            version: Some("v[1]".into()),
            authors: vec!["Alice".into(), "Bob".into()],
            url: Some("https://example.com/a(b)?x=1&y=2".into()),
            filename: "example.jar.disabled".into(),
            mod_id: Some("example".into()),
        }];
        let fields = Fields {
            version: true,
            authors: true,
            url: true,
            filename: true,
        };
        let get = |format, template| render(&entries, format, fields, template);
        assert!(get(Format::Html, "").contains(
            "<a href=\"https://example.com/a(b)?x=1&amp;y=2\">A, &quot;&lt;mod&gt;&quot; {version}</a>"
        ));
        assert!(get(Format::Markdown, "").contains("](https://example.com/a%28b%29?x=1&y=2)"));
        assert!(get(Format::Text, "").ends_with(" by Alice, Bob (example.jar.disabled)"));
        assert_eq!(get(Format::Csv, ""), "\"A, \"\"<mod>\"\" {version}\",https://example.com/a(b)?x=1&y=2,v[1],\"Alice,Bob\",example.jar.disabled");
        let json: serde_json::Value = serde_json::from_str(&get(Format::Json, "")).unwrap();
        assert_eq!(json[0]["authors"], serde_json::json!(["Alice", "Bob"]));
        assert_eq!(json[0]["filename"], "example.jar.disabled");
        assert_eq!(
            get(Format::Custom, "{mod_id}: {name} | {version} | {authors} | {url} | {filename}"),
            "example: A, \"<mod>\" {version} | v[1] | Alice, Bob | https://example.com/a(b)?x=1&y=2 | example.jar.disabled"
        );
        let bare = render(&entries, Format::Json, Fields::default(), "");
        let bare: serde_json::Value = serde_json::from_str(&bare).unwrap();
        assert_eq!(bare, serde_json::json!([{ "name": entries[0].name }]));
    }

    #[test]
    fn csv_neutralizes_formula_prefixes_in_each_column() {
        let mut entry = ModListEntry {
            name: "=name,other".into(),
            url: Some("+url".into()),
            version: Some("-version".into()),
            authors: vec!["@author".into(), "Another \"Author\"".into()],
            filename: "\tfile.jar".into(),
            mod_id: None,
        };
        let fields = Fields {
            url: true,
            version: true,
            authors: true,
            filename: true,
        };
        assert_eq!(
            render(std::slice::from_ref(&entry), Format::Csv, fields, ""),
            "\"'=name,other\",'+url,'-version,\"'@author,Another \"\"Author\"\"\",'\tfile.jar"
        );

        entry.name = "\rreturn".into();
        assert_eq!(
            render(&[entry], Format::Csv, Fields::default(), ""),
            "\"'\rreturn\""
        );
    }

    #[test]
    fn lists_disabled_jars_with_embedded_authors_and_packwiz_project_urls() {
        let mods = tempfile::tempdir().unwrap();
        let mut jar = zip::ZipWriter::new(
            std::fs::File::create(mods.path().join("real.jar.disabled")).unwrap(),
        );
        jar.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        jar.write_all(br#"{"name":"Embedded","id":"example","version":"1.2","authors":["Alice",{"name":"Bob"}]}"#).unwrap();
        jar.finish().unwrap();
        std::fs::create_dir(mods.path().join(".index")).unwrap();
        std::fs::write(
            mods.path().join(".index/example.pw.toml"),
            r#"
name = "Indexed"
filename = "real.jar"
[download]
hash-format = "sha1"
hash = "example"
[update.modrinth]
mod-id = "AB12"
version = "opaque-id"
"#,
        )
        .unwrap();
        let entries = collect(mods.path());
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "Embedded");
        assert_eq!(entry.version.as_deref(), Some("1.2"));
        assert_eq!(entry.mod_id.as_deref(), Some("example"));
        assert_eq!(entry.authors, ["Alice", "Bob"]);
        assert_eq!(entry.url.as_deref(), Some("https://modrinth.com/mod/AB12"));
        assert_eq!(entry.filename, "real.jar.disabled");
    }

    #[test]
    fn indexed_provider_ids_are_not_release_versions() {
        let mods = tempfile::tempdir().unwrap();
        let index = mods.path().join(".index");
        std::fs::create_dir(&index).unwrap();
        for (name, update) in [
            (
                "modrinth",
                "[update.modrinth]\nmod-id = \"AB12\"\nversion = \"opaque-id\"",
            ),
            (
                "curseforge",
                "[update.curseforge]\nproject-id = 12\nfile-id = 12345",
            ),
        ] {
            std::fs::File::create(mods.path().join(format!("{name}.jar"))).unwrap();
            std::fs::write(
                index.join(format!("{name}.pw.toml")),
                format!(
                    "name = \"{name}\"\nfilename = \"{name}.jar\"\n[download]\nhash-format = \"sha1\"\nhash = \"example\"\n{update}"
                ),
            )
            .unwrap();
        }
        let entries = collect(mods.path());
        assert_eq!(entries.len(), 2);
        for entry in entries {
            assert!(
                entry.url.is_some(),
                "index should be matched for {}",
                entry.name
            );
            assert_eq!(entry.version, None, "provider ID for {}", entry.name);
        }
    }

    #[test]
    fn html_declares_utf8_for_non_ascii_metadata() {
        let entry = ModListEntry {
            name: "Café 日本語".into(),
            url: None,
            version: None,
            authors: Vec::new(),
            filename: "example.jar".into(),
            mod_id: None,
        };
        assert_eq!(
            render(&[entry], Format::Html, Fields::default(), ""),
            "<html><head><meta charset=\"utf-8\"></head><body><ul>\n\t<li>Café 日本語</li>\n</ul></body></html>"
        );
    }

    #[test]
    fn html_preview_does_not_link_to_script_urls() {
        let entry = ModListEntry {
            name: "<script>".into(),
            url: Some("javascript:alert(1)".into()),
            version: None,
            authors: Vec::new(),
            filename: "example.jar".into(),
            mod_id: None,
        };
        let output = render(
            &[entry],
            Format::Html,
            Fields {
                url: true,
                ..Fields::default()
            },
            "",
        );
        assert!(output.contains("&lt;script&gt;"));
        assert!(!output.contains("javascript:"));
    }
}
