use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent, extract_info_hash, create_magnet, parse_size};

const WATCHSOMUCH_BASE: &str = "https://watchsomuch.com";

// WatchSoMuch sometimes has a JSON API
#[derive(Debug, Deserialize)]
struct WSMQueryResponse {
    #[serde(rename = "movie_results")]
    movies: Option<Vec<WSMMovie>>,
    #[serde(rename = "tv_results")]
    series: Option<Vec<WSMSeries>>,
}

#[derive(Debug, Deserialize)]
struct WSMMovie {
    id: i64,
    title: String,
    #[serde(rename = "release_date")]
    year: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WSMSeries {
    id: i64,
    name: String,
    #[serde(rename = "first_air_date")]
    year: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WSMTorrent {
    hash: String,
    title: String,
    size: String,
    seeders: i32,
    leechers: i32,
    quality: String,
    category: String,
    magnet: Option<String>,
}

pub struct WatchSoMuchScraper {
    client: Client,
}

impl WatchSoMuchScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        
        Ok(Self { client })
    }

    async fn search_html(&self, query: &str) -> Result<Vec<ScrapedTorrent>> {
        let search_url = format!("{}/search?q={}", WATCHSOMUCH_BASE, urlencoding::encode(query));
        
        let response = self.client
            .get(&search_url)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("WatchSoMuch returned status: {}", response.status()));
        }

        let html = response.text().await?;
        let document = Html::parse_document(&html);
        
        let mut torrents = Vec::new();
        
        // WatchSoMuch uses a results table
        let row_selector = Selector::parse("table.results-table tbody tr").unwrap();
        let td_selector = Selector::parse("td").unwrap();
        let a_selector = Selector::parse("a").unwrap();
        
        for row in document.select(&row_selector) {
            let tds: Vec<_> = row.select(&td_selector).collect();
            
            if tds.len() < 5 {
                continue;
            }
            
            // Extract title and magnet
            let title_cell = &tds[0];
            let title = title_cell
                .select(&a_selector)
                .next()
                .and_then(|a| a.text().next())
                .unwrap_or("")
                .trim()
                .to_string();
            
            let magnet_link = title_cell
                .select(&a_selector)
                .filter_map(|a| a.value().attr("href"))
                .find(|href| href.starts_with("magnet:"))
                .unwrap_or("")
                .to_string();
            
            let info_hash = if !magnet_link.is_empty() {
                extract_info_hash(&magnet_link).unwrap_or_default()
            } else {
                continue;
            };
            
            // Extract size
            let size_str = tds.get(1)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            
            let size_bytes = parse_size(&size_str);
            
            // Extract seeders
            let seeders = tds.get(3)
                .and_then(|td| td.text().next())
                .and_then(|t| t.parse::<i32>().ok())
                .unwrap_or(0);
            
            // Extract leechers
            let leechers = tds.get(4)
                .and_then(|td| td.text().next())
                .and_then(|t| t.parse::<i32>().ok())
                .unwrap_or(0);
            
            let category = "Movies/TV".to_string();
            
            torrents.push(ScrapedTorrent {
                title,
                info_hash,
                magnet_link,
                size_bytes,
                size_gb: size_bytes as f64 / 1_073_741_824.0,
                seeders,
                leechers,
                source: "WatchSoMuch".to_string(),
                category,
            });
        }
        
        Ok(torrents)
    }

    async fn search_api(&self, query: &str, content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        // Try the API endpoint if available
        let endpoint = match content_type {
            "movie" => "movies",
            "series" => "tv",
            _ => "search",
        };
        
        let api_url = format!("{}/api/{}/{}?limit=20", WATCHSOMUCH_BASE, endpoint, urlencoding::encode(query));
        
        let response = self.client
            .get(&api_url)
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<Vec<WSMTorrent>>().await {
                    Ok(api_results) => {
                        let mut torrents = Vec::new();
                        
                        for result in api_results {
                            let info_hash = result.hash.to_lowercase();
                            let magnet_link = result.magnet.unwrap_or_else(|| {
                                create_magnet(&info_hash, &result.title)
                            });
                            
                            let size_bytes = parse_size(&result.size);
                            
                            torrents.push(ScrapedTorrent {
                                title: result.title,
                                info_hash,
                                magnet_link,
                                size_bytes,
                                size_gb: size_bytes as f64 / 1_073_741_824.0,
                                seeders: result.seeders,
                                leechers: result.leechers,
                                source: "WatchSoMuch".to_string(),
                                category: result.category,
                            });
                        }
                        
                        Ok(torrents)
                    }
                    Err(_) => Ok(Vec::new()),
                }
            }
            _ => Ok(Vec::new()),
        }
    }
}

#[async_trait]
impl Scraper for WatchSoMuchScraper {
    fn name(&self) -> &str {
        "WatchSoMuch"
    }

    async fn search(&self, query: &str, content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        // Try API first, then fallback to HTML scraping
        match self.search_api(query, content_type).await {
            Ok(torrents) if !torrents.is_empty() => Ok(torrents),
            _ => self.search_html(query).await,
        }
    }

    fn supports_anime(&self) -> bool {
        false
    }

    fn supports_movies(&self) -> bool {
        true
    }

    fn supports_series(&self) -> bool {
        true
    }
}