use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent};

// SolidTorrents API - Public API
// Docs: https://solidtorrents.to/api
const SOLID_API: &str = "https://solidtorrents.to/api/v1";

#[derive(Debug, Deserialize)]
struct SolidResponse {
    results: Vec<SolidTorrent>,
}

#[derive(Debug, Deserialize)]
struct SolidTorrent {
    title: String,
    #[serde(rename = "hash")]
    info_hash: String,
    #[serde(rename = "magnet")]
    magnet_link: String,
    #[serde(rename = "size")]
    size_bytes: u64,
    seeders: i32,
    leechers: i32,
    category: String,
}

pub struct SolidScraper {
    client: Client,
}

impl SolidScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;
        
        Ok(Self { client })
    }

    pub async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        tracing::info!("SolidTorrents search for: {}", query);
        
        let search_url = format!(
            "{}/search?q={}&sort=seeders&f-seeders=1",
            SOLID_API,
            urlencoding::encode(query)
        );
        
        let response = match self.client.get(&search_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!("SolidTorrents API request failed: {}", e);
                return Ok(Vec::new());
            }
        };

        if !response.status().is_success() {
            tracing::error!("SolidTorrents API returned status: {}", response.status());
            return Ok(Vec::new());
        }

        let data: SolidResponse = match response.json().await {
            Ok(data) => data,
            Err(e) => {
                tracing::error!("SolidTorrents API JSON parse failed: {}", e);
                return Ok(Vec::new());
            }
        };
        
        tracing::info!("SolidTorrents returned {} results", data.results.len());
        
        let mut scraped = Vec::new();
        
        for torrent in data.results {
            if torrent.seeders == 0 {
                continue;
            }
            
            scraped.push(ScrapedTorrent {
                title: torrent.title,
                info_hash: torrent.info_hash.to_lowercase(),
                magnet_link: torrent.magnet_link,
                size_bytes: torrent.size_bytes,
                size_gb: (torrent.size_bytes as f64) / 1_073_741_824.0,
                seeders: torrent.seeders,
                leechers: torrent.leechers,
                source: "SolidTorrents".to_string(),
                category: torrent.category,
            });
        }
        
        Ok(scraped)
    }
}

#[async_trait]
impl Scraper for SolidScraper {
    fn name(&self) -> &str {
        "SolidTorrents"
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
