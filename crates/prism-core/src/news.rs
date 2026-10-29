//! News feed: RSS parsing of the PrismLauncher news feed.
//! Mirrors `launcher/news/NewsChecker.cpp`.

use crate::net::Downloader;
use std::sync::Arc;

pub const NEWS_RSS_URL: &str = "https://prismlauncher.org/feed.xml";

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

fn all_entries(xml: &str) -> Vec<NewsEntry> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(item_start) = rest.find("<item>") {
        let item_end = rest[item_start..].find("</item>").map(|e| item_start + e + 7);
        let Some(item_end) = item_end else { break };
        let item = &rest[item_start..item_end];
        out.push(NewsEntry {
            title: first_element(item, "title").unwrap_or_default(),
            link: first_element(item, "link").unwrap_or_default(),
            pub_date: first_element(item, "pubDate").unwrap_or_default(),
            content: first_element(item, "description")
                .or_else(|| first_element(item, "content:encoded"))
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
}
