use async_trait::async_trait;
use reqwest::Client;
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent, parse_size};
use quick_xml::events::Event;
use quick_xml::reader::Reader;

const NYAA_BASE: &str = "https://nyaa.si";
const SUKEBEI_BASE: &str = "https://sukebei.nyaa.si";

pub struct NyaaScraper {
    client: Client,
}

#[derive(Debug, Default)]
struct RSSTorrent {
    title: String,
    info_hash: String,
    size: String,
    seeders: String,
    leechers: String,
    category: String,
}

impl NyaaScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()?;
        
        Ok(Self { client })
    }

    async fn search_nyaa_rss(&self, query: &str, is_nsfw: bool) -> Result<Vec<ScrapedTorrent>> {
        let base_url = if is_nsfw { SUKEBEI_BASE } else { NYAA_BASE };
        let search_url = format!("{}/?page=rss&q={}", base_url, urlencoding::encode(query));
        
        tracing::info!("Nyaa RSS search: {}", search_url);
        
        let response = self.client
            .get(&search_url)
            .send()
            .await?;

        if !response.status().is_success() {
            tracing::error!("Nyaa RSS returned status: {}", response.status());
            return Err(anyhow::anyhow!("Nyaa RSS returned status: {}", response.status()));
        }

        let rss_content = response.text().await?;
        tracing::info!("Nyaa RSS returned {} bytes", rss_content.len());
        
        let torrents = self.parse_nyaa_rss(&rss_content, is_nsfw)?;
        tracing::info!("Nyaa RSS parsed {} torrents", torrents.len());
        
        Ok(torrents)
    }

    fn parse_nyaa_rss(&self, rss_content: &str, is_nsfw: bool) -> Result<Vec<ScrapedTorrent>> {
        let mut torrents = Vec::new();
        let mut reader = Reader::from_str(rss_content);
        reader.trim_text(true);
        
        let mut current_item = RSSTorrent::default();
        let mut current_tag = String::new();
        let mut in_item = false;
        
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    let name_bytes = e.local_name().as_ref().to_vec();
                    let name = String::from_utf8_lossy(&name_bytes);
                    
                    // Check if it's an item
                    if name == "item" {
                        in_item = true;
                        current_item = RSSTorrent::default();
                    }
                    
                    current_tag = name.to_string();
                }
                Ok(Event::End(e)) => {
                    let name_bytes = e.local_name().as_ref().to_vec();
                    let name = String::from_utf8_lossy(&name_bytes);
                    
                    if name == "item" {
                        // Process completed item
                        if !current_item.title.is_empty() && !current_item.info_hash.is_empty() {
                            let size_bytes = parse_size(&current_item.size);
                            let seeders = current_item.seeders.parse::<i32>().unwrap_or(0);
                            let leechers = current_item.leechers.parse::<i32>().unwrap_or(0);
                            
                            let magnet_link = format!("magnet:?xt=urn:btih:{}", current_item.info_hash);
                            
                            torrents.push(ScrapedTorrent {
                                title: current_item.title.clone(),
                                info_hash: current_item.info_hash.to_lowercase(),
                                magnet_link,
                                size_bytes,
                                size_gb: size_bytes as f64 / 1_073_741_824.0,
                                seeders,
                                leechers,
                                source: if is_nsfw { "Sukebei".to_string() } else { "Nyaa".to_string() },
                                category: current_item.category.clone(),
                            });
                        }
                        in_item = false;
                    }
                    
                    current_tag.clear();
                }
                Ok(Event::Text(e)) => {
                    if in_item {
                        let text_bytes = e.as_ref().to_vec();
                        let text = String::from_utf8_lossy(&text_bytes);
                        
                        // Handle nyaa namespace fields - quick-xml strips the namespace prefix
                        match current_tag.as_str() {
                            "title" => current_item.title = text.to_string(),
                            "infoHash" => current_item.info_hash = text.to_string(),
                            "size" => current_item.size = text.to_string(),
                            "seeders" => current_item.seeders = text.to_string(),
                            "leechers" => current_item.leechers = text.to_string(),
                            "category" => current_item.category = text.to_string(),
                            _ => {}
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    tracing::warn!("Error parsing RSS: {}", e);
                    break;
                }
                _ => {}
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
        
        // Search both Nyaa and Sukebei in parallel using RSS
        let nyaa_future = self.search_nyaa_rss(query, false);
        let sukebei_future = self.search_nyaa_rss(query, true);
        
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
