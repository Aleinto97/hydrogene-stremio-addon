use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent, extract_info_hash, create_magnet, parse_size};

// Rutor mirrors
const RUTOR_MIRRORS: &[&str] = &[
    "http://rutor.info",
    "http://rutor.is",
    "http://rutorc6mqdinc4cz.onion",
];

pub struct RutorScraper {
    client: Client,
}

impl RutorScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        
        Ok(Self { client })
    }

    async fn search_with_mirror(&self, query: &str, mirror: &str) -> Result<Vec<ScrapedTorrent>> {
        let search_url = format!("{}/search/0/0/000/2/{}", mirror, urlencoding::encode(query));
        
        let response = self.client
            .get(&search_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Rutor returned status: {}", response.status()));
        }

        let html = response.text().await?;
        let document = Html::parse_document(&html);
        
        let mut torrents = Vec::new();
        
        // Rutor uses a table with class "gai" or "tum"
        let row_selector = Selector::parse("div#index table tr.gai, div#index table tr.tum").unwrap();
        let td_selector = Selector::parse("td").unwrap();
        let a_selector = Selector::parse("a").unwrap();
        
        for row in document.select(&row_selector) {
            let tds: Vec<_> = row.select(&td_selector).collect();
            
            if tds.len() < 4 {
                continue;
            }
            
            // Extract title and magnet from the second column
            let title_cell = &tds[1];
            let links: Vec<_> = title_cell.select(&a_selector).collect();
            
            if links.len() < 2 {
                continue;
            }
            
            let title = links.last()
                .and_then(|a| a.text().next())
                .unwrap_or("")
                .trim()
                .to_string();
            
            // Rutor uses torrent file links - we need to extract info_hash from magnet if available
            // or construct from torrent page
            let magnet_link = links.iter()
                .filter_map(|a| a.value().attr("href"))
                .find(|href| href.starts_with("magnet:"))
                .unwrap_or("")
                .to_string();
            
            let info_hash = if !magnet_link.is_empty() {
                extract_info_hash(&magnet_link).unwrap_or_default()
            } else {
                // Try to get from torrent file link
                links.iter()
                    .filter_map(|a| a.value().attr("href"))
                    .find(|href| href.contains(".torrent"))
                    .and_then(|href| {
                        // Extract hash from download link
                        // Rutor format: /download/12345
                        href.split('/').nth(2).map(|s| s.to_string())
                    })
                    .unwrap_or_default()
            };
            
            if title.is_empty() || info_hash.is_empty() {
                continue;
            }
            
            // If no magnet link, create one
            let magnet_link = if magnet_link.is_empty() {
                create_magnet(&info_hash, &title)
            } else {
                magnet_link
            };
            
            // Extract size
            let size_str = tds.get(2)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            
            let size_bytes = parse_size(&size_str);
            
            // Extract seeders from the last column (format: S | L)
            let peers_text = tds.last()
                .map(|td| td.text().collect::<String>())
                .unwrap_or_default();
            
            let (seeders, leechers) = Self::parse_peers(&peers_text);
            
            // Category from first column
            let category = tds.get(0)
                .and_then(|td| td.select(&a_selector).next())
                .and_then(|a| a.value().attr("href"))
                .and_then(|href| href.split('/').nth(2))
                .unwrap_or("Unknown")
                .to_string();
            
            torrents.push(ScrapedTorrent {
                title,
                info_hash,
                magnet_link,
                size_bytes,
                size_gb: size_bytes as f64 / 1_073_741_824.0,
                seeders,
                leechers,
                source: "Rutor".to_string(),
                category,
            });
        }
        
        Ok(torrents)
    }

    fn parse_peers(text: &str) -> (i32, i32) {
        let parts: Vec<&str> = text.split('|').collect();
        let seeders = parts.get(0)
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0);
        let leechers = parts.get(1)
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0);
        (seeders, leechers)
    }
}

#[async_trait]
impl Scraper for RutorScraper {
    fn name(&self) -> &str {
        "Rutor"
    }

    async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        // Try mirrors in order
        for mirror in RUTOR_MIRRORS {
            match self.search_with_mirror(query, mirror).await {
                Ok(torrents) if !torrents.is_empty() => return Ok(torrents),
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        
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