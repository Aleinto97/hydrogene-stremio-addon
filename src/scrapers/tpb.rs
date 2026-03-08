use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent, create_magnet};

// The Pirate Bay has multiple mirrors - we'll try them in order
const TPB_MIRRORS: &[&str] = &[
    "https://apibay.org",
    "https://piratebay.live",
    "https://tpb.party",
];

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
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        
        Ok(Self { client })
    }

    async fn search_with_mirror(&self, query: &str, mirror: &str) -> Result<Vec<ScrapedTorrent>> {
        let search_url = format!("{}/q.php?q={}&cat=0", mirror, urlencoding::encode(query));
        
        let response = self.client
            .get(&search_url)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("TPB mirror returned status: {}", response.status()));
        }

        let results: Vec<TPBResult> = response.json().await?;
        
        let mut torrents = Vec::new();
        
        for result in results {
            // Skip if ID is "0" (no results)
            if result.id == "0" {
                continue;
            }
            
            let size_bytes: u64 = result.size.parse().unwrap_or(0);
            let seeders: i32 = result.seeders.parse().unwrap_or(0);
            let leechers: i32 = result.leechers.parse().unwrap_or(0);
            
            if seeders == 0 && result.id != "0" {
                // API sometimes returns 0 for all seeders - skip if dead
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
            });
        }
        
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
        "The Pirate Bay"
    }

    async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        // Try mirrors in order until one works
        for mirror in TPB_MIRRORS {
            match self.search_with_mirror(query, mirror).await {
                Ok(torrents) if !torrents.is_empty() => return Ok(torrents),
                Ok(_) => continue, // Empty results, try next mirror
                Err(_) => continue, // Error, try next mirror
            }
        }
        
        // All mirrors failed
        Ok(Vec::new())
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