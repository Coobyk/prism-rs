//! News feed: RSS parsing of the PrismLauncher news feed.
//! Mirrors `launcher/news/NewsChecker.cpp`.

use crate::net::Downloader;
use std::sync::Arc;

pub const NEWS_RSS_URL: &str = "https://prismlauncher.org/feed/feed.xml";

#[derive(Debug, Clone, Default)]
pub struct NewsEntry {
    pub title: String,
    pub link: String,
    pub pub_date: String,
    pub content: String,
}

/// Extract the text content of the first matching element.
fn first_element(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = xml.find(&open)? + open.len();
    let start = xml[start..].find('>')? + start + 1;
    let close = format!("</{tag}>");
    let end = start + xml[start..].find(&close)?;
    Some(xml[start..end].to_string())
}

/// First `href` attribute of a link element (Atom feeds).
fn first_link_href(xml: &str) -> Option<String> {
    let start = xml.find("<link")? + 5;
    let end = xml[start..].find('>')? + start;
    let tag = &xml[start..end];
    let href_start = tag.find("href=\"")? + 6;
    let rest = &tag[href_start..];
    let href_end = rest.find('"')?;
    Some(rest[..href_end].to_string())
}

fn all_entries(xml: &str) -> Vec<NewsEntry> {
    // Atom (<entry>) and RSS (<item>) both supported.
    let (tag, items) = if xml.contains("<entry>") {
        ("entry", true)
    } else {
        ("item", true)
    };
    let _ = items;
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(item_start) = rest.find(&open) {
        let Some(item_end) = rest[item_start..].find(&close).map(|e| item_start + e + close.len()) else {
            break;
        };
        let item = &rest[item_start..item_end];
        let link = if tag == "entry" {
            first_link_href(item).unwrap_or_default()
        } else {
            first_element(item, "link").unwrap_or_default()
        };
        let date = first_element(item, "pubDate")
            .or_else(|| first_element(item, "updated"))
            .unwrap_or_default();
        out.push(NewsEntry {
            title: first_element(item, "title").unwrap_or_default(),
            link,
            pub_date: date,
            content: first_element(item, "description")
                .or_else(|| first_element(item, "content:encoded"))
                .or_else(|| first_element(item, "content"))
                .unwrap_or_default(),
        });
        rest = &rest[item_end..];
    }
    out
}

/// Fetch and parse the news feed. Offline-safe: returns an empty list on
/// failure.
pub async fn fetch_news(downloader: &Arc<Downloader>) -> Vec<NewsEntry> {
    match downloader.get_text(NEWS_RSS_URL).await {
        Ok(xml) => all_entries(&xml),
        Err(e) => {
            tracing::warn!("news fetch failed: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_items() {
        let xml = r#"<rss><channel>
            <item><title>Prism Launcher 9.0</title><link>https://prismlauncher.org/9</link>
            <pubDate>Mon, 01 Jan 2026 00:00:00 +0000</pubDate><description>Hello</description></item>
            <item><title>Second</title><link>https://prismlauncher.org/8</link></item>
        </channel></rss>"#;
        let items = all_entries(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Prism Launcher 9.0");
        assert_eq!(items[0].content, "Hello");
        assert_eq!(items[1].link, "https://prismlauncher.org/8");
    }

    #[test]
    fn parses_atom_entries() {
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom">
            <entry><title>Release 1</title><link rel="alternate" href="https://prismlauncher.org/r1"/>
            <updated>2026-01-01T00:00:00Z</updated><content>News body</content></entry>
        </feed>"#;
        let items = all_entries(xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Release 1");
        assert_eq!(items[0].link, "https://prismlauncher.org/r1");
        assert_eq!(items[0].content, "News body");
    }
}
