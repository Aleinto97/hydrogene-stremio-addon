use crate::scrapers::{parse_size, ScrapedTorrent, Scraper};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};

const RUTRACKER_BASE: &str = "https://rutracker.org/forum";

pub struct RuTrackerScraper {
    client: Client,
    session_cookie: String,
}

impl RuTrackerScraper {
    pub fn new() -> Result<Self> {
        let session_cookie = std::env::var("RUTRACKER_COOKIE").unwrap_or_default();

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        Ok(Self {
            client,
            session_cookie,
        })
    }

    async fn search_rutracker(&self, query: &str) -> Result<Vec<ScrapedTorrent>> {
        if self.session_cookie.is_empty() {
            return Ok(Vec::new());
        }

        let search_url = format!("{}/tracker.php", RUTRACKER_BASE);

        let form_data = [
            ("nm", query),
            ("o", "10"), // Sort by seeders
            ("s", "2"),  // Descending
            ("tm", "-1"),
            ("pn", ""),
            ("f[]", "-1"),
        ];

        let cookie_header = format!("bb_session={}", self.session_cookie);

        let response = self
            .client
            .post(&search_url)
            .header("Cookie", cookie_header)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .form(&form_data)
            .send()
            .await?;

        if response.status().as_u16() == 302 {
            // Redirect to login - session expired or invalid
            return Err(anyhow::anyhow!("RuTracker session expired or invalid"));
        }

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "RuTracker returned status: {}",
                response.status()
            ));
        }

        let html = response.text().await?;
        let document = Html::parse_document(&html);

        let mut torrents = Vec::new();

        // RuTracker uses table with class "forumline"
        let row_selector = Selector::parse("table.forumline tr").unwrap();
        let td_selector = Selector::parse("td").unwrap();
        let a_selector = Selector::parse("a").unwrap();

        for row in document.select(&row_selector) {
            let tds: Vec<_> = row.select(&td_selector).collect();

            // RuTracker results have a specific structure
            if tds.len() < 10 {
                continue;
            }

            // Extract forum/category from first column
            let category = tds
                .get(1)
                .and_then(|td| td.select(&a_selector).next())
                .and_then(|a| a.text().next())
                .unwrap_or("Unknown")
                .to_string();

            // Extract title and topic link
            let title_cell = tds.get(3);
            if title_cell.is_none() {
                continue;
            }

            let title_cell = title_cell.unwrap();
            let title_links: Vec<_> = title_cell.select(&a_selector).collect();

            let title = title_links
                .first()
                .and_then(|a| a.text().next())
                .unwrap_or("")
                .trim()
                .to_string();

            if title.is_empty() {
                continue;
            }

            // Extract topic ID for magnet link
            let topic_url = title_links
                .first()
                .and_then(|a| a.value().attr("href"))
                .unwrap_or("");

            let topic_id = topic_url.split('=').nth(1).unwrap_or("");

            if topic_id.is_empty() {
                continue;
            }

            // Extract size
            let size_str = tds
                .get(5)
                .map(|td| td.text().collect::<String>().trim().replace("&nbsp;", " "))
                .unwrap_or_default();

            let size_bytes = parse_size(&size_str);

            // Extract seeders and leechers
            let seeders = tds
                .get(6)
                .map(|td| td.text().collect::<String>())
                .and_then(|s| s.trim().replace(",", "").parse::<i32>().ok())
                .unwrap_or(0);

            let leechers = tds
                .get(7)
                .map(|td| td.text().collect::<String>())
                .and_then(|s| s.trim().replace(",", "").parse::<i32>().ok())
                .unwrap_or(0);

            // For RuTracker, we need to fetch the topic page to get the magnet
            // For now, create a placeholder - in production you'd fetch the topic
            let info_hash = format!("rutracker_{}", topic_id); // Placeholder
            let magnet_link = format!(
                "magnet:?xt=urn:btih:{}&dn={}",
                info_hash,
                urlencoding::encode(&title)
            );

            torrents.push(ScrapedTorrent {
                title,
                info_hash,
                magnet_link,
                size_bytes,
                size_gb: size_bytes as f64 / 1_073_741_824.0,
                seeders,
                leechers,
                source: "RuTracker".to_string(),
                category,
            });
        }

        Ok(torrents)
    }

    // Helper to fetch actual magnet from topic page (expensive operation)
    #[allow(dead_code)]
    async fn fetch_magnet_from_topic(&self, topic_id: &str) -> Result<Option<String>> {
        let topic_url = format!("{}/viewtopic.php?t={}", RUTRACKER_BASE, topic_id);

        let cookie_header = format!("bb_session={}", self.session_cookie);

        let response = self
            .client
            .get(&topic_url)
            .header("Cookie", cookie_header)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .send()
            .await?;

        let html = response.text().await?;
        let document = Html::parse_document(&html);

        let magnet_selector = Selector::parse("a[href^='magnet:']").unwrap();

        for element in document.select(&magnet_selector) {
            if let Some(href) = element.value().attr("href") {
                return Ok(Some(href.to_string()));
            }
        }

        Ok(None)
    }
}

#[async_trait]
impl Scraper for RuTrackerScraper {
    fn name(&self) -> &str {
        "RuTracker"
    }

    async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        if self.session_cookie.is_empty() {
            tracing::info!("RuTracker cookie not configured, skipping");
            return Ok(Vec::new());
        }

        self.search_rutracker(query).await
    }

    fn supports_anime(&self) -> bool {
        true
    }

    fn supports_movies(&self) -> bool {
        true
    }

    fn supports_series(&self) -> bool {
        true
    }
}
