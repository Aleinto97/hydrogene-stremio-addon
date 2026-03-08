use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent, extract_info_hash, create_magnet, parse_size};

const NYAA_BASE: &str = "https://nyaa.si";
const SUKEBEI_BASE: &str = "https://sukebei.nyaa.si";

pub struct NyaaScraper {
    client: Client,
}

impl NyaaScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        
        Ok(Self { client })
    }

    async fn search_nyaa(&self, query: &str, is_nsfw: bool) -> Result<Vec<ScrapedTorrent>> {
        let base_url = if is_nsfw { SUKEBEI_BASE } else { NYAA_BASE };
        let search_url = format!("{}/?f=0&c=0_0&q={}", base_url, urlencoding::encode(query));
        
        let response = self.client
            .get(&search_url)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Nyaa returned status: {}", response.status()));
        }

        let html = response.text().await?;
        let document = Html::parse_document(&html);
        
        let mut torrents = Vec::new();
        
        // Nyaa table rows
        let row_selector = Selector::parse("table.torrent-list tbody tr").unwrap();
        let td_selector = Selector::parse("td").unwrap();
        let a_selector = Selector::parse("a").unwrap();
        
        for row in document.select(&row_selector) {
            let tds: Vec<_> = row.select(&td_selector).collect();
            
            if tds.len() < 6 {
                continue;
            }
            
            // Extract category
            let category = tds.get(0)
                .and_then(|td| td.select(&a_selector).next())
                .and_then(|a| a.value().attr("title"))
                .unwrap_or("Unknown")
                .to_string();
            
            // Extract title and magnet link
            let title_cell = &tds[1];
            let links: Vec<_> = title_cell.select(&a_selector).collect();
            
            if links.is_empty() {
                continue;
            }
            
            let title = links.last()
                .and_then(|a| a.text().next())
                .unwrap_or("")
                .to_string();
            
            // Get magnet link from first <a> that has href starting with "magnet:"
            let magnet_link = links.iter()
                .filter_map(|a| a.value().attr("href"))
                .find(|href| href.starts_with("magnet:"))
                .unwrap_or("")
                .to_string();
            
            if magnet_link.is_empty() {
                continue;
            }
            
            let info_hash = extract_info_hash(&magnet_link)
                .unwrap_or_default();
            
            // Extract size
            let size_str = tds.get(3)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            
            let size_bytes = parse_size(&size_str);
            
            // Extract seeders/leechers
            let seeders = tds.get(5)
                .and_then(|td| td.text().next())
                .and_then(|t| t.parse::<i32>().ok())
                .unwrap_or(0);
            
            let leechers = tds.get(6)
                .and_then(|td| td.text().next())
                .and_then(|t| t.parse::<i32>().ok())
                .unwrap_or(0);
            
            if !title.is_empty() && !info_hash.is_empty() {
                torrents.push(ScrapedTorrent {
                    title,
                    info_hash,
                    magnet_link,
                    size_bytes,
                    size_gb: size_bytes as f64 / 1_073_741_824.0,
                    seeders,
                    leechers,
                    source: if is_nsfw { "Sukebei".to_string() } else { "Nyaa".to_string() },
                    category,
                });
            }
        }
        
        Ok(torrents)
    }
}

#[async_trait]
impl Scraper for NyaaScraper {
    fn name(&self) -> &str {
        "Nyaa/Sukebei"
    }

    async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        let mut results = Vec::new();
        
        // Search both Nyaa and Sukebei in parallel
        let nyaa_future = self.search_nyaa(query, false);
        let sukebei_future = self.search_nyaa(query, true);
        
        let (nyaa_result, sukebei_result) = tokio::join!(nyaa_future, sukebei_future);
        
        if let Ok(mut torrents) = nyaa_result {
            results.append(&mut torrents);
        }
        
        if let Ok(mut torrents) = sukebei_result {
            results.append(&mut torrents);
        }
        
        Ok(results)
    }

    fn supports_anime(&self) -> bool {
        true
    }

    fn supports_movies(&self) -> bool {
        false
    }

    fn supports_series(&self) -> bool {
        true
    }
}