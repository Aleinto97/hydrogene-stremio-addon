use crate::scrapers::{create_magnet, ScrapedTorrent, Scraper};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

// YTS.mx API - Official JSON API for movie torrents
// Docs: https://yts.mx/api
const YTS_MIRRORS: &[&str] = &[
    "https://yts.mx/api/v2",
    "https://yts.pm/api/v2",
    "https://yts.rs/api/v2",
    "https://yts.lt/api/v2",
];

#[derive(Debug, Deserialize)]
struct YtsResponse {
    data: YtsData,
}

#[derive(Debug, Deserialize)]
struct YtsData {
    movies: Option<Vec<YtsMovie>>,
}

#[derive(Debug, Deserialize)]
struct YtsMovie {
    title: String,
    #[serde(rename = "title_long")]
    title_long: String,
    year: i32,
    #[serde(rename = "imdb_code")]
    imdb_code: String,
    torrents: Vec<YtsTorrent>,
}

#[derive(Debug, Deserialize)]
struct YtsTorrent {
    url: String,
    hash: String,
    quality: String,
    #[serde(rename = "type")]
    torrent_type: String,
    seeds: i32,
    peers: i32,
    size: String,
    #[serde(rename = "size_bytes")]
    size_bytes: i64,
}

pub struct YtsScraper {
    client: Client,
}

impl YtsScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;

        Ok(Self { client })
    }

    pub async fn search(&self, query: &str, _content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        for mirror in YTS_MIRRORS {
            match self.search_with_mirror(query, mirror).await {
                Ok(torrents) if !torrents.is_empty() => return Ok(torrents),
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!("YTS mirror {} failed: {}", mirror, e);
                    continue;
                }
            }
        }
        Ok(Vec::new())
    }

    pub async fn search_with_mirror(
        &self,
        query: &str,
        mirror: &str,
    ) -> Result<Vec<ScrapedTorrent>> {
        tracing::info!("YTS API search for: {} on {}", query, mirror);

        // Build API URL - supports both query and IMDB ID
        let search_url = if query.starts_with("tt") {
            // Search by IMDB ID
            format!(
                "{}/list_movies.json?query_term={}&limit=50&sort_by=seeds&order_by=desc",
                mirror, query
            )
        } else {
            // Regular search
            format!(
                "{}/list_movies.json?query_term={}&limit=50&sort_by=seeds&order_by=desc",
                mirror,
                urlencoding::encode(query)
            )
        };

        let response = match self.client.get(&search_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!("YTS API request failed: {}", e);
                return Ok(Vec::new());
            }
        };

        if !response.status().is_success() {
            tracing::error!("YTS API returned status: {}", response.status());
            return Ok(Vec::new());
        }

        let yts_response: YtsResponse = match response.json().await {
            Ok(data) => data,
            Err(e) => {
                tracing::error!("YTS API JSON parse failed: {}", e);
                return Ok(Vec::new());
            }
        };

        let movies = yts_response.data.movies.unwrap_or_default();
        tracing::info!("YTS API returned {} movies", movies.len());

        let mut scraped = Vec::new();

        for movie in movies {
            for torrent in movie.torrents {
                // Skip dead torrents (0 seeders)
                if torrent.seeds == 0 {
                    continue;
                }

                let info_hash = torrent.hash.to_lowercase();
                let magnet_link = create_magnet(&info_hash, &movie.title_long);

                let category = format!("Movies/{}p", torrent.quality);

                scraped.push(ScrapedTorrent {
                    title: format!("{} ({})", movie.title_long, torrent.quality),
                    info_hash,
                    magnet_link,
                    size_bytes: torrent.size_bytes as u64,
                    size_gb: (torrent.size_bytes as f64) / 1_073_741_824.0,
                    seeders: torrent.seeds,
                    leechers: torrent.peers,
                    source: "YTS".to_string(),
                    category,
                });
            }
        }

        tracing::info!("YTS filtered to {} valid torrents", scraped.len());
        Ok(scraped)
    }
}

#[async_trait]
impl Scraper for YtsScraper {
    fn name(&self) -> &str {
        "YTS"
    }

    async fn search(&self, query: &str, content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        // Only search for movies, not series
        if content_type == "series" {
            return Ok(Vec::new());
        }
        self.search(query, content_type).await
    }

    fn supports_anime(&self) -> bool {
        false
    }

    fn supports_movies(&self) -> bool {
        true
    }

    fn supports_series(&self) -> bool {
        false // YTS is movies only
    }
}
