use async_trait::async_trait;
use std::collections::HashSet;
use anyhow::Result;
use tracing::info;

pub mod nyaa;
pub mod tpb;
pub mod rutor;
pub mod rutracker;
pub mod x1337;
pub mod bitsearch;
pub mod yts;
pub mod eztv;
pub mod nekobt;
pub mod solidtorrents;

#[derive(Debug, Clone)]
pub struct ScrapedTorrent {
    pub title: String,
    pub info_hash: String,
    pub magnet_link: String,
    pub size_bytes: u64,
    pub size_gb: f64,
    pub seeders: i32,
    pub leechers: i32,
    pub source: String,
    pub category: String,
    pub is_cached: bool,
}

#[async_trait]
pub trait Scraper: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &str, content_type: &str) -> Result<Vec<ScrapedTorrent>>;
    fn supports_anime(&self) -> bool;
    fn supports_movies(&self) -> bool;
    fn supports_series(&self) -> bool;
}

pub struct ScraperManager {
    scrapers: Vec<Box<dyn Scraper>>,
}

impl ScraperManager {
    pub fn new() -> Result<Self> {
        let scrapers: Vec<Box<dyn Scraper>> = vec![
            Box::new(nyaa::NyaaScraper::new()?),
            Box::new(tpb::TPBScraper::new()?),
            Box::new(x1337::X1337Scraper::new()?),
            Box::new(rutor::RutorScraper::new()?),
            Box::new(rutracker::RuTrackerScraper::new()?),
            Box::new(bitsearch::BitsearchScraper::new()?),
            Box::new(yts::YtsScraper::new()?),
            Box::new(eztv::EztvScraper::new()?),
            Box::new(nekobt::NekoBtScraper::new()?),
            Box::new(solidtorrents::SolidScraper::new()?),
        ];

        Ok(Self { scrapers })
    }

    pub async fn scrape_all(&self, id: &str, content_type: &str) -> Vec<ScrapedTorrent> {
        use futures::stream::{FuturesUnordered, StreamExt};
        use tokio::time::{timeout, Duration};
        use tracing::warn;

        let query = Self::id_to_query(id);
        let scraper_timeout = Duration::from_secs(15);

        let target_scrapers: Vec<_> = self.scrapers
            .iter()
            .filter(|s| {
                match content_type {
                    "movie" => s.supports_movies(),
                    "series" => s.supports_series(),
                    _ => true,
                }
            })
            .collect();

        let mut stream = FuturesUnordered::new();

        for scraper in &target_scrapers {
            let name = scraper.name().to_string();
            let query = query.clone();
            let content_type = content_type.to_string();

            let future = async move {
                match timeout(scraper_timeout, scraper.search(&query, &content_type)).await {
                    Ok(Ok(results)) => {
                        info!(scraper = %name, count = results.len(), "completed successfully");
                        results
                    }
                    Ok(Err(e)) => {
                        warn!(scraper = %name, error = %e, "scraper failed");
                        vec![]
                    }
                    Err(_) => {
                        warn!(scraper = %name, timeout_secs = 1, "scraper timed out");
                        vec![]
                    }
                }
            };
            stream.push(future);
        }

        let mut all_torrents: Vec<ScrapedTorrent> = Vec::new();
        
        while let Some(scraper_results) = stream.next().await {
            all_torrents.extend(scraper_results);
        }

        let mut seen_hashes = HashSet::new();
        all_torrents.retain(|t| seen_hashes.insert(t.info_hash.clone()));
        
        all_torrents.sort_by(|a, b| b.seeders.cmp(&a.seeders));
        all_torrents.truncate(50);
        
        info!(total = all_torrents.len(), "scraping completed");
        all_torrents
    }

    fn id_to_query(id: &str) -> String {
        // Handle IMDB IDs (tt1234567)
        if id.starts_with("tt") {
            // For now, return the ID as-is; in production, 
            // you'd want to look up the actual title from IMDB
            id.to_string()
        } else if id.starts_with("anilist:") {
            // Handle AniList anime IDs
            id.replace("anilist:", "")
        } else {
            id.to_string()
        }
    }
}

// Helper function to parse size strings
pub fn parse_size(size_str: &str) -> u64 {
    let size_str = size_str.to_uppercase().replace(",", "");
    let parts: Vec<&str> = size_str.split_whitespace().collect();
    
    if parts.len() != 2 {
        return 0;
    }
    
    let value: f64 = parts[0].parse().unwrap_or(0.0);
    let unit = parts[1];
    
    let multiplier = match unit {
        "B" => 1.0,
        "KB" | "KIB" => 1024.0,
        "MB" | "MIB" => 1024.0 * 1024.0,
        "GB" | "GIB" => 1024.0 * 1024.0 * 1024.0,
        "TB" | "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    
    (value * multiplier) as u64
}

// Helper function to extract info_hash from magnet link
pub fn extract_info_hash(magnet: &str) -> Option<String> {
    if let Some(start) = magnet.find("xt=urn:btih:") {
        let hash_start = start + 12;
        let hash_end = magnet[hash_start..].find('&').map(|i| hash_start + i).unwrap_or(magnet.len());
        return Some(magnet[hash_start..hash_end].to_lowercase());
    }
    None
}

// Helper function to create magnet link from info_hash and name
pub fn create_magnet(info_hash: &str, name: &str) -> String {
    let encoded_name = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
    format!("magnet:?xt=urn:btih:{}&dn={}", info_hash.to_lowercase(), encoded_name)
}