use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use anyhow::Result;
use crate::scrapers::{ScrapedTorrent, Scraper};

// EZTV.re API - Official JSON API for TV torrents
// Simple endpoint that returns latest torrents
const EZTV_API: &str = "https://eztv.re/api/get-torrents";
const EZTV_SITE: &str = "https://eztv.re";

#[derive(Debug, Deserialize)]
struct EztvResponse {
    #[serde(rename = "torrents_count")]
    count: i32,
    #[serde(rename = "torrents")]
    torrents: Vec<EztvTorrent>,
}

#[derive(Debug, Deserialize, Clone)]
struct EztvTorrent {
    #[serde(rename = "torrent_id")]
    id: i64,
    #[serde(rename = "torrent_hash")]
    hash: String,
    #[serde(rename = "torrent_title")]
    title: String,
    #[serde(rename = "imdb_id")]
    imdb_id: String,
    #[serde(rename = "season")]
    season: String,
    #[serde(rename = "episode")]
    episode: String,
    #[serde(rename = "seeds")]
    seeders: i32,
    #[serde(rename = "peers")]
    leechers: i32,
    #[serde(rename = "date_released_unix")]
    date_released: i64,
    #[serde(rename = "size_bytes")]
    size_bytes: String,
}

pub struct EztvScraper {
    client: Client,
}

impl EztvScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;
        
        Ok(Self { client })
    }

    pub async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        tracing::info!("EZTV API search for: {}", query);
        
        let search_url = if query.starts_with("tt") {
            // Search by IMDB ID
            format!("{}?imdb_id={}&limit=100", EZTV_API, query.trim_start_matches("tt"))
        } else {
            // Search by query text (EZTV doesn't have a text search API, so we get recent and filter)
            format!("{}?limit=100", EZTV_API)
        };
        
        let response = match self.client
            .get(&search_url)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!("EZTV API request failed: {}", e);
                return Ok(Vec::new());
            }
        };

        if !response.status().is_success() {
            tracing::error!("EZTV API returned status: {}", response.status());
            return Ok(Vec::new());
        }

        let eztv_data: EztvResponse = match response.json().await {
            Ok(data) => data,
            Err(e) => {
                tracing::error!("EZTV API JSON parse failed: {}", e);
                return Ok(Vec::new());
            }
        };
        
        tracing::info!("EZTV API returned {} torrents", eztv_data.count);
        
        let mut scraped = Vec::new();
        let query_lower = query.to_lowercase();
        
        for torrent in eztv_data.torrents {
            // If not searching by IMDB, filter by query text
            if !query.starts_with("tt") && !torrent.title.to_lowercase().contains(&query_lower) {
                continue;
            }
            
            // Skip dead torrents (0 seeders)
            if torrent.seeders == 0 {
                continue;
            }
            
            let info_hash = torrent.hash.to_lowercase();
            let size_bytes: u64 = torrent.size_bytes.parse().unwrap_or(0);
            let magnet_link = format!(
                "magnet:?xt=urn:btih:{}&dn={}",
                info_hash,
                urlencoding::encode(&torrent.title)
            );
            
            // Format season/episode info
            let episode_info = if !torrent.season.is_empty() && !torrent.episode.is_empty() {
                format!(" S{}E{}", torrent.season, torrent.episode)
            } else {
                String::new()
            };
            
            let category = if !episode_info.is_empty() {
                "TV/Episode".to_string()
            } else {
                "TV".to_string()
            };
            
            scraped.push(ScrapedTorrent {
                title: format!("{}{}", torrent.title, episode_info),
                info_hash,
                magnet_link,
                size_bytes,
                size_gb: (size_bytes as f64) / 1_073_741_824.0,
                seeders: torrent.seeders,
                leechers: torrent.leechers,
                source: "EZTV".to_string(),
                category,
                is_cached: false,
            });
        }
        
        tracing::info!("EZTV filtered to {} valid torrents", scraped.len());
        Ok(scraped)
    }
}

#[async_trait]
impl Scraper for EztvScraper {
    fn name(&self) -> &str {
        "EZTV"
    }

    async fn search(&self, query: &str, content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        // EZTV is TV shows only
        if content_type == "movie" {
            return Ok(Vec::new());
        }
        self.search(query, content_type).await
    }

    fn supports_anime(&self) -> bool {
        false
    }

    fn supports_movies(&self) -> bool {
        false  // EZTV is TV only
    }

    fn supports_series(&self) -> bool {
        true
    }
}
