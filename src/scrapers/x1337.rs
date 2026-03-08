use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use anyhow::Result;
use crate::scrapers::{Scraper, ScrapedTorrent, extract_info_hash, parse_size};

// 1337x mirrors - often less protected than main domain
const X1337_MIRRORS: &[&str] = &[
    "https://1337x.to",
    "https://1337x.st",
    "https://x1337x.ws",
];

// FlareSolverr endpoint
const FLARESOLVERR_URL: &str = "http://localhost:8191/v1";

pub struct X1337Scraper {
    client: Client,
    flaresolverr_enabled: bool,
}

#[derive(Debug, Serialize)]
struct FlareSolverrRequest {
    cmd: String,
    url: String,
    max_timeout: i32,
}

#[derive(Debug, Deserialize)]
struct FlareSolverrResponse {
    status: String,
    message: String,
    solution: Option<FlareSolverrSolution>,
}

#[derive(Debug, Deserialize)]
struct FlareSolverrSolution {
    url: String,
    status: u16,
    #[serde(rename = "response")]
    html: String,
}

impl X1337Scraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        
        // Check if FlareSolverr is enabled via env var
        let flaresolverr_enabled = std::env::var("USE_FLARESOLVERR")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true); // Default to true since 1337x needs it
        
        Ok(Self { client, flaresolverr_enabled })
    }

    async fn fetch_with_flaresolverr(&self, url: &str) -> Result<String> {
        let payload = FlareSolverrRequest {
            cmd: "request.get".to_string(),
            url: url.to_string(),
            max_timeout: 60000,
        };
        
        tracing::info!("Using FlareSolverr for: {}", url);
        
        let response = self.client
            .post(FLARESOLVERR_URL)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("FlareSolverr returned status: {}", response.status()));
        }

        let result: FlareSolverrResponse = response.json().await?;
        
        if result.status != "ok" {
            return Err(anyhow::anyhow!("FlareSolverr error: {}", result.message));
        }
        
        let solution = result.solution
            .ok_or_else(|| anyhow::anyhow!("No solution in FlareSolverr response"))?;
        
        if solution.status != 200 {
            return Err(anyhow::anyhow!("FlareSolverr solution returned status: {}", solution.status));
        }
        
        Ok(solution.html)
    }

    async fn fetch_direct(&self, url: &str) -> Result<String> {
        let response = self.client
            .get(url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("HTTP {}", response.status()));
        }

        Ok(response.text().await?)
    }

    async fn fetch_html(&self, url: &str) -> Result<String> {
        if self.flaresolverr_enabled {
            match self.fetch_with_flaresolverr(url).await {
                Ok(html) => return Ok(html),
                Err(e) => {
                    tracing::warn!("FlareSolverr failed: {}, trying direct request", e);
                    return self.fetch_direct(url).await;
                }
            }
        } else {
            self.fetch_direct(url).await
        }
    }

    async fn search_with_mirror(&self, query: &str, mirror: &str) -> Result<Vec<ScrapedTorrent>> {
        let search_url = format!("{}/search/{}/1/", mirror, urlencoding::encode(query));
        
        let html = match self.fetch_html(&search_url).await {
            Ok(html) => html,
            Err(e) => {
                tracing::warn!("Failed to fetch search page from {}: {}", mirror, e);
                return Ok(Vec::new());
            }
        };
        
        // Parse all torrent info first (synchronously)
        let torrent_info = self.parse_search_page(&html, mirror);
        
        if torrent_info.is_empty() {
            tracing::warn!("No torrents found on search page from {}", mirror);
        }
        
        // Now fetch magnets for each torrent
        let mut torrents = Vec::new();
        for (title, details_url, size_bytes, seeders, leechers, uploader) in torrent_info {
            if let Ok(magnet) = self.fetch_magnet_from_details(&details_url).await {
                if let Some(info_hash) = extract_info_hash(&magnet) {
                    torrents.push(ScrapedTorrent {
                        title,
                        info_hash,
                        magnet_link: magnet,
                        size_bytes,
                        size_gb: size_bytes as f64 / 1_073_741_824.0,
                        seeders,
                        leechers,
                        source: "1337x".to_string(),
                        category: format!("uploader: {}", uploader),
                        is_cached: false,
                    });
                }
            }
        }
        
        Ok(torrents)
    }

    fn parse_search_page(&self, html: &str, mirror: &str) -> Vec<(String, String, u64, i32, i32, String)> {
        let document = Html::parse_document(html);
        let mut results = Vec::new();
        
        // 1337x uses table with class "table-list"
        let row_selector = Selector::parse("table.table-list tbody tr").unwrap();
        let td_selector = Selector::parse("td").unwrap();
        let a_selector = Selector::parse("a").unwrap();
        
        for row in document.select(&row_selector) {
            let tds: Vec<_> = row.select(&td_selector).collect();
            
            if tds.len() < 6 {
                continue;
            }
            
            // First column contains name and link
            let name_cell = &tds[0];
            let name_links: Vec<_> = name_cell.select(&a_selector).collect();
            
            // Get the torrent details link (usually second <a>)
            let details_link = name_links.get(1)
                .and_then(|a| a.value().attr("href"))
                .map(|href| format!("{}{}", mirror, href));
            
            let title = name_links.get(1)
                .and_then(|a| a.text().next())
                .unwrap_or("")
                .trim()
                .to_string();
            
            if title.is_empty() || details_link.is_none() {
                continue;
            }
            
            // Get seeders/leechers
            let seeders = tds.get(1)
                .and_then(|td| td.text().next())
                .and_then(|t| t.parse::<i32>().ok())
                .unwrap_or(0);
            
            let leechers = tds.get(2)
                .and_then(|td| td.text().next())
                .and_then(|t| t.parse::<i32>().ok())
                .unwrap_or(0);
            
            // Get size
            let size_str = tds.get(4)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            
            let size_bytes = parse_size(&size_str);
            
            // Get uploader
            let uploader = tds.get(5)
                .and_then(|td| td.select(&a_selector).next())
                .and_then(|a| a.text().next())
                .unwrap_or("Unknown")
                .to_string();
            
            results.push((title, details_link.unwrap(), size_bytes, seeders, leechers, uploader));
        }
        
        results
    }

    async fn fetch_magnet_from_details(&self, details_url: &str) -> Result<String> {
        let html = match self.fetch_html(details_url).await {
            Ok(html) => html,
            Err(e) => {
                tracing::warn!("Failed to fetch details page {}: {}", details_url, e);
                return Err(e);
            }
        };
        
        let document = Html::parse_document(&html);
        
        // Look for magnet link
        let magnet_selector = Selector::parse("a[href^='magnet:']").unwrap();
        
        for element in document.select(&magnet_selector) {
            if let Some(href) = element.value().attr("href") {
                return Ok(href.to_string());
            }
        }
        
        Err(anyhow::anyhow!("No magnet link found"))
    }
}

#[async_trait]
impl Scraper for X1337Scraper {
    fn name(&self) -> &str {
        "1337x"
    }

    async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        // Try mirrors in order
        for mirror in X1337_MIRRORS {
            match self.search_with_mirror(query, mirror).await {
                Ok(torrents) if !torrents.is_empty() => return Ok(torrents),
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!("Mirror {} failed: {}", mirror, e);
                    continue;
                }
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
