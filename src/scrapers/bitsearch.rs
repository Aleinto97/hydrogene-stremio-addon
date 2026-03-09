use crate::scrapers::{ScrapedTorrent, Scraper};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

// Bitsearch.to API - Free tier: 200 requests/day without API key
// Docs: https://bitsearch.to/api
const BITSEARCH_API: &str = "https://bitsearch.to/api";

#[derive(Debug, Deserialize)]
struct BitsearchResponse {
    torrents: Vec<BitsearchTorrent>,
}

#[derive(Debug, Deserialize)]
struct BitsearchTorrent {
    title: String,
    #[serde(rename = "magnet")]
    magnet_link: String,
    #[serde(rename = "size")]
    size_bytes: i64,
    seeders: i32,
    leechers: i32,
    #[serde(rename = "category")]
    category_str: String,
    // Info hash is not provided directly, extract from magnet
}

pub struct BitsearchScraper {
    client: Client,
    api_key: Option<String>,
}

impl BitsearchScraper {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("BITSEARCH_API_KEY").ok();

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;

        Ok(Self { client, api_key })
    }

    pub async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        tracing::info!("Bitsearch API search for: {}", query);

        // Build URL with optional API key
        let search_url = if let Some(ref key) = self.api_key {
            format!(
                "{}/search?q={}&api_key={}",
                BITSEARCH_API,
                urlencoding::encode(query),
                key
            )
        } else {
            format!("{}/search?q={}", BITSEARCH_API, urlencoding::encode(query))
        };

        let response = match self.client.get(&search_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!("Bitsearch API request failed: {}", e);
                return Ok(Vec::new());
            }
        };

        if !response.status().is_success() {
            tracing::error!("Bitsearch API returned status: {}", response.status());
            return Ok(Vec::new());
        }

        let data: BitsearchResponse = match response.json().await {
            Ok(data) => data,
            Err(e) => {
                tracing::error!("Bitsearch API JSON parse failed: {}", e);
                return Ok(Vec::new());
            }
        };

        tracing::info!("Bitsearch API returned {} results", data.torrents.len());

        let mut scraped = Vec::new();

        for torrent in data.torrents {
            // Extract info_hash from magnet link
            let info_hash = extract_hash_from_magnet(&torrent.magnet_link);

            if info_hash.is_empty() {
                continue;
            }

            // Skip dead torrents (0 seeders)
            if torrent.seeders == 0 {
                continue;
            }

            let category = Self::normalize_category(&torrent.category_str);

            scraped.push(ScrapedTorrent {
                title: torrent.title,
                info_hash: info_hash.clone(),
                magnet_link: torrent.magnet_link,
                size_bytes: torrent.size_bytes as u64,
                size_gb: (torrent.size_bytes as f64) / 1_073_741_824.0,
                seeders: torrent.seeders,
                leechers: torrent.leechers,
                source: "Bitsearch".to_string(),
                category,
            });
        }

        tracing::info!("Bitsearch filtered to {} valid torrents", scraped.len());
        Ok(scraped)
    }

    fn normalize_category(cat: &str) -> String {
        match cat.to_lowercase().as_str() {
            "movies" => "Movies",
            "tv" | "television" => "TV",
            "music" => "Music",
            "games" => "Games",
            "software" => "Software",
            "anime" => "Anime",
            "books" | "ebooks" => "Books",
            _ => "Other",
        }
        .to_string()
    }
}

fn extract_hash_from_magnet(magnet: &str) -> String {
    if let Some(start) = magnet.find("xt=urn:btih:") {
        let hash_start = start + 12;
        let hash_end = magnet[hash_start..]
            .find('&')
            .map(|i| hash_start + i)
            .unwrap_or(magnet.len());
        let hash = &magnet[hash_start..hash_end];
        // Handle both hex and base32 hashes
        if hash.len() == 40 || hash.len() == 32 {
            return hash.to_lowercase();
        }
    }
    String::new()
}

#[async_trait]
impl Scraper for BitsearchScraper {
    fn name(&self) -> &str {
        "Bitsearch"
    }

    async fn search(&self, query: &str, content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        self.search(query, content_type).await
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
