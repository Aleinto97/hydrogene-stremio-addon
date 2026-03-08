use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent, extract_info_hash, create_magnet};

// NekoBT.to - Anime torrent tracker
// Public search: https://nekobt.to/search?q={query}
// API (with key): https://nekobt.to/api/v1/torrents?apikey={key}&q={query}
const NEKO_BASE: &str = "https://nekobt.to";
const NEKO_API: &str = "https://nekobt.to/api/v1";

#[derive(Debug, Deserialize)]
struct NekoApiResponse {
    #[serde(default)]
    torrents: Vec<NekoTorrent>,
    #[serde(default)]
    total: i32,
}

#[derive(Debug, Deserialize)]
struct NekoTorrent {
    id: i64,
    title: String,
    hash: String,
    #[serde(rename = "magnet_link")]
    magnet: Option<String>,
    size: i64,
    seeders: i32,
    leechers: i32,
    #[serde(default)]
    media_title: Option<String>,
}

pub struct NekoBtScraper {
    client: Client,
    api_key: Option<String>,
}

impl NekoBtScraper {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("NEKOBT_API_KEY").ok();
        
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;
        
        Ok(Self { client, api_key })
    }

    pub async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        tracing::info!("NekoBT search for: {}", query);
        
        // If API key is available, use the official API
        if let Some(ref key) = self.api_key {
            return self.search_api(query, key).await;
        }
        
        // Otherwise, fallback to HTML scraping
        self.search_html(query).await
    }

    async fn search_api(&self, query: &str, api_key: &str) -> Result<Vec<ScrapedTorrent>> {
        tracing::info!("NekoBT using API with key");
        
        let api_url = format!(
            "{}/torrents?apikey={}&q={}&limit=50&sort=seeders&order=desc",
            NEKO_API,
            urlencoding::encode(api_key),
            urlencoding::encode(query)
        );
        
        let response = match self.client.get(&api_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!("NekoBT API request failed: {}", e);
                return self.search_html(query).await; // Fallback to HTML
            }
        };

        if !response.status().is_success() {
            tracing::error!("NekoBT API returned status: {}", response.status());
            return self.search_html(query).await; // Fallback to HTML
        }

        let api_data: NekoApiResponse = match response.json().await {
            Ok(data) => data,
            Err(e) => {
                tracing::error!("NekoBT API JSON parse failed: {}", e);
                return self.search_html(query).await; // Fallback to HTML
            }
        };
        
        tracing::info!("NekoBT API returned {} torrents", api_data.torrents.len());
        
        let mut torrents = Vec::new();
        
        for torrent in api_data.torrents {
            if torrent.seeders == 0 {
                continue;
            }
            
            let info_hash = torrent.hash.to_lowercase();
            let magnet_link = torrent.magnet.unwrap_or_else(|| {
                create_magnet(&info_hash, &torrent.title)
            });
            
            torrents.push(ScrapedTorrent {
                title: torrent.title,
                info_hash,
                magnet_link,
                size_bytes: torrent.size as u64,
                size_gb: (torrent.size as f64) / 1_073_741_824.0,
                seeders: torrent.seeders,
                leechers: torrent.leechers,
                source: "NekoBT".to_string(),
                category: "Anime".to_string(),
                is_cached: false,
            });
        }
        
        Ok(torrents)
    }

    async fn search_html(&self, query: &str) -> Result<Vec<ScrapedTorrent>> {
        tracing::info!("NekoBT using HTML scraping");
        
        let search_url = format!("{}/search?q={}", NEKO_BASE, urlencoding::encode(query));
        
        let response = match self.client
            .get(&search_url)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!("NekoBT request failed: {}", e);
                return Ok(Vec::new());
            }
        };

        if !response.status().is_success() {
            tracing::error!("NekoBT returned status: {}", response.status());
            return Ok(Vec::new());
        }

        let html = match response.text().await {
            Ok(html) => html,
            Err(e) => {
                tracing::error!("NekoBT HTML parse failed: {}", e);
                return Ok(Vec::new());
            }
        };

        let document = Html::parse_document(&html);
        let mut torrents = Vec::new();

        // Try multiple selectors for torrent rows
        let row_selectors = [
            "table tbody tr",
            ".torrent-row",
            ".search-result",
            "[data-torrent]",
        ];

        let mut found_rows = false;
        
        for selector_str in &row_selectors {
            let row_selector = match Selector::parse(selector_str) {
                Ok(s) => s,
                Err(_) => continue,
            };

            for row in document.select(&row_selector) {
                found_rows = true;
                
                let title_selector = Selector::parse("a").unwrap();
                let title_elem = row.select(&title_selector)
                    .find(|a| {
                        a.value().attr("href")
                            .map(|href| href.contains("/torrent/"))
                            .unwrap_or(false)
                    });

                let title = title_elem
                    .and_then(|a| a.text().next())
                    .map(|t| t.trim().to_string())
                    .unwrap_or_default();

                if title.is_empty() {
                    continue;
                }

                let magnet_link = row.select(&Selector::parse("a[href^='magnet:']").unwrap())
                    .next()
                    .and_then(|a| a.value().attr("href"))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        row.select(&Selector::parse("a").unwrap())
                            .filter_map(|a| a.value().attr("href"))
                            .find(|href| href.starts_with("magnet:"))
                            .map(|s| s.to_string())
                            .unwrap_or_default()
                    });

                let info_hash = if !magnet_link.is_empty() {
                    extract_info_hash(&magnet_link).unwrap_or_default()
                } else {
                    String::new()
                };

                if title.is_empty() || info_hash.is_empty() {
                    continue;
                }

                let size_text = row.text().collect::<String>();
                let size_bytes = Self::extract_size(&size_text);
                let seeders = Self::extract_number(&row, &["seeders", "seeds", "↑"], 0);
                let leechers = Self::extract_number(&row, &["leechers", "leeches", "↓"], 0);

                if seeders == 0 {
                    continue;
                }

                let magnet_final = if magnet_link.is_empty() {
                    create_magnet(&info_hash, &title)
                } else {
                    magnet_link
                };

                torrents.push(ScrapedTorrent {
                    title,
                    info_hash,
                    magnet_link: magnet_final,
                    size_bytes,
                    size_gb: size_bytes as f64 / 1_073_741_824.0,
                    seeders,
                    leechers,
                    source: "NekoBT".to_string(),
                    category: "Anime".to_string(),
                    is_cached: false,
                });
            }

            if !torrents.is_empty() {
                break;
            }
        }

        if !found_rows {
            tracing::warn!("NekoBT: No torrent rows found, trying alternative parsing");
            
            let link_selector = Selector::parse("a[href*='magnet:']").unwrap();
            for link in document.select(&link_selector) {
                if let Some(magnet) = link.value().attr("href") {
                    if let Some(info_hash) = extract_info_hash(magnet) {
                        let title = link.text().next()
                            .map(|t| t.trim().to_string())
                            .unwrap_or_else(|| "Unknown".to_string());
                        
                        if title != "Unknown" {
                            torrents.push(ScrapedTorrent {
                                title,
                                info_hash: info_hash.clone(),
                                magnet_link: magnet.to_string(),
                                size_bytes: 0,
                                size_gb: 0.0,
                                seeders: 1,
                                leechers: 0,
                                source: "NekoBT".to_string(),
                                category: "Anime".to_string(),
                                is_cached: false,
                            });
                        }
                    }
                }
            }
        }

        tracing::info!("NekoBT found {} torrents", torrents.len());
        Ok(torrents)
    }

    fn extract_number(row: &scraper::ElementRef, keywords: &[&str], default: i32) -> i32 {
        let text = row.text().collect::<String>();
        
        for keyword in keywords {
            if let Some(pos) = text.to_lowercase().find(keyword) {
                let after = &text[pos + keyword.len()..];
                if let Some(num) = after.chars()
                    .skip_while(|c| !c.is_ascii_digit())
                    .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
                    .collect::<String>()
                    .replace(',', "")
                    .replace('.', "")
                    .parse::<i32>()
                    .ok() {
                    return num;
                }
            }
        }
        
        default
    }

    fn extract_size(text: &str) -> u64 {
        let size_regex = match regex::Regex::new(r"(\d+\.?\d*)\s*(GB?|MB?|TB?|GiB?|MiB?|KiB?)") {
            Ok(r) => r,
            Err(_) => return 0,
        };
        
        if let Some(caps) = size_regex.captures(text) {
            let value: f64 = caps.get(1)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0.0);
            let unit = caps.get(2)
                .map(|m| m.as_str().to_uppercase())
                .unwrap_or_default();
            
            let multiplier = match unit.as_str() {
                "B" => 1.0,
                "KB" | "K" | "KIB" => 1024.0,
                "MB" | "M" | "MIB" => 1024.0 * 1024.0,
                "GB" | "G" | "GIB" => 1024.0 * 1024.0 * 1024.0,
                "TB" | "T" | "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
                _ => 1.0,
            };
            
            return (value * multiplier) as u64;
        }
        
        0
    }
}

#[async_trait]
impl Scraper for NekoBtScraper {
    fn name(&self) -> &str {
        "NekoBT"
    }

    async fn search(&self, query: &str, content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        if content_type == "movie" {
            return Ok(Vec::new());
        }
        self.search(query, content_type).await
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
