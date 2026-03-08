use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent, create_magnet};

// The Pirate Bay official API - no blocks, no Cloudflare
const TPB_API: &str = "https://apibay.org";

#[derive(Debug, Deserialize)]
struct TPBResult {
    id: String,
    name: String,
    info_hash: String,
    leechers: String,
    seeders: String,
    num_files: String,
    size: String,
    #[serde(rename = "category")]
    category_id: String,
    #[serde(rename = "added")]
    date_added: String,
    status: String,
    #[serde(rename = "imdb")]
    imdb_id: Option<String>,
}

pub struct TPBScraper {
    client: Client,
}

impl TPBScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()?;
        
        Ok(Self { client })
    }

    pub async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        tracing::info!("TPB API search for: {}", query);
        
        let search_url = format!("{}/q.php?q={}&cat=0", TPB_API, urlencoding::encode(query));
        
        let response = match self.client
            .get(&search_url)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!("TPB API request failed: {}", e);
                return Ok(Vec::new());
            }
        };

        if !response.status().is_success() {
            tracing::error!("TPB API returned status: {}", response.status());
            return Ok(Vec::new());
        }

        let results: Vec<TPBResult> = match response.json().await {
            Ok(data) => data,
            Err(e) => {
                tracing::error!("TPB API JSON parse failed: {}", e);
                return Ok(Vec::new());
            }
        };
        
        tracing::info!("TPB API returned {} results", results.len());
        
        let mut torrents = Vec::new();
        
        for result in results {
            // Skip if ID is "0" (no results marker)
            if result.id == "0" {
                continue;
            }
            
            let size_bytes: u64 = result.size.parse().unwrap_or(0);
            let seeders: i32 = result.seeders.parse().unwrap_or(0);
            let leechers: i32 = result.leechers.parse().unwrap_or(0);
            
            // Skip dead torrents (0 seeders)
            if seeders == 0 {
                continue;
            }
            
            let info_hash = result.info_hash.to_lowercase();
            let magnet_link = create_magnet(&info_hash, &result.name);
            
            let category = Self::category_name(&result.category_id);
            
            torrents.push(ScrapedTorrent {
                title: result.name,
                info_hash,
                magnet_link,
                size_bytes,
                size_gb: size_bytes as f64 / 1_073_741_824.0,
                seeders,
                leechers,
                source: "TPB".to_string(),
                category,
                is_cached: false,
            });
        }
        
        tracing::info!("TPB filtered to {} valid torrents", torrents.len());
        Ok(torrents)
    }

    fn category_name(cat_id: &str) -> String {
        match cat_id {
            "201" => "Video/Movies",
            "202" => "Video/Movies DVDR",
            "203" => "Video/Music videos",
            "204" => "Video/Movie clips",
            "205" => "Video/TV shows",
            "206" => "Video/Handheld",
            "207" => "Video/HD - Movies",
            "208" => "Video/HD - TV shows",
            "209" => "Video/3D",
            "210" => "Video/Other",
            _ => "Unknown",
        }.to_string()
    }
}

#[async_trait]
impl Scraper for TPBScraper {
    fn name(&self) -> &str {
        "The Pirate Bay (API)"
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