use reqwest::Client;
use serde::Deserialize;
use anyhow::{Result, anyhow};
use std::sync::Arc;

const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";
const OMDB_API_BASE: &str = "http://www.omdbapi.com";
const ANILIST_API_BASE: &str = "https://graphql.anilist.co";

pub struct MetadataClient {
    client: Arc<Client>,
    tmdb_api_key: Option<String>,
    omdb_api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TMDBSearchResult {
    results: Vec<TMDBMovie>,
}

#[derive(Debug, Deserialize)]
struct TMDBMovie {
    id: i64,
    title: Option<String>,
    name: Option<String>,
    #[serde(rename = "original_title")]
    original_title: Option<String>,
    #[serde(rename = "original_name")]
    original_name: Option<String>,
    #[serde(rename = "release_date")]
    release_date: Option<String>,
    #[serde(rename = "first_air_date")]
    first_air_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OMDBResult {
    title: Option<String>,
    #[serde(rename = "Year")]
    year: Option<String>,
    #[serde(rename = "Type")]
    content_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ContentMetadata {
    pub title: String,
    pub year: Option<String>,
    pub content_type: String,
    pub search_queries: Vec<String>,
}

impl MetadataClient {
    pub fn new(client: Arc<Client>) -> Result<Self> {
        let tmdb_api_key = std::env::var("TMDB_API_KEY").ok();
        let omdb_api_key = std::env::var("OMDB_API_KEY").ok();

        Ok(Self {
            client,
            tmdb_api_key,
            omdb_api_key,
        })
    }

    /// Resolve anime ID (anilist:) to title using GraphQL
    pub async fn resolve_anime_title(&self, stremio_id: &str) -> Option<String> {
        let parts: Vec<&str> = stremio_id.split(':').collect();
        
        match parts.as_slice() {
            // --- CASE: ANILIST via GraphQL ---
            ["anilist", id] | ["anilist", id, _] => {
                let anime_id = id.parse::<i32>().unwrap_or(0);
                
                if anime_id == 0 {
                    return None;
                }
                
                let query = serde_json::json!({
                    "query": "query ($id: Int) { Media (id: $id, type: ANIME) { title { english romaji } } }",
                    "variables": { "id": anime_id }
                });
                
                match self.client
                    .post(ANILIST_API_BASE)
                    .header("Content-Type", "application/json")
                    .json(&query)
                    .send()
                    .await
                {
                    Ok(res) if res.status().is_success() => {
                        match res.json::<serde_json::Value>().await {
                            Ok(data) => {
                                // Try English title first, fallback to romaji
                                let title = data["data"]["Media"]["title"]["english"]
                                    .as_str()
                                    .or_else(|| data["data"]["Media"]["title"]["romaji"].as_str());
                                
                                if let Some(t) = title {
                                    return Some(t.to_string());
                                }
                            }
                            Err(e) => {
                                tracing::warn!("AniList JSON parse error: {}", e);
                            }
                        }
                    }
                    Ok(res) => {
                        tracing::warn!("AniList bad status: {}", res.status());
                    }
                    Err(e) => {
                        tracing::warn!("AniList request error: {}", e);
                    }
                }
            }
            
            _ => {}
        }
        
        None
    }

    pub async fn lookup_by_imdb(&self, imdb_id: &str, content_type: &str) -> Result<ContentMetadata> {
        // Parse season and episode from IMDB ID if present (format: tt1234567:1:3)
        let id_parts: Vec<&str> = imdb_id.split(':').collect();
        let (base_imdb_id, season, episode) = if id_parts.len() >= 3 {
            let base = id_parts[0].to_string();
            let season = id_parts[1].parse::<u32>().ok();
            let episode = id_parts[2].parse::<u32>().ok();
            (base, season, episode)
        } else {
            (imdb_id.to_string(), None, None)
        };
        
        // Handle anime IDs (anilist:) via GraphQL
        if imdb_id.starts_with("anilist:") {
            tracing::info!("Resolving anime ID via GraphQL: {}", imdb_id);
            
            match self.resolve_anime_title(imdb_id).await {
                Some(title) => {
                    tracing::info!("Anime resolved to title: {}", title);
                    return Ok(ContentMetadata {
                        title: title.clone(),
                        year: None,
                        content_type: "series".to_string(),
                        search_queries: vec![title],
                    });
                }
                None => {
                    tracing::warn!("Failed to resolve anime ID: {}", imdb_id);
                }
            }
            
            // Fallback: use ID for direct search
            let fallback_id = imdb_id.replace("anilist:", "");
            
            return Ok(ContentMetadata {
                title: format!("Anime {}", fallback_id),
                year: None,
                content_type: "series".to_string(),
                search_queries: vec![fallback_id],
            });
        }

        // Try TMDB for movies/series
        if let Some(ref api_key) = self.tmdb_api_key {
            if let Ok(mut metadata) = self.lookup_tmdb(&base_imdb_id, content_type, api_key).await {
                // Add season/episode specific queries for series
                if let (Some(season_num), Some(episode_num)) = (season, episode) {
                    let title = &metadata.title;
                    // Add SXXEYY queries for better torrent matching
                    metadata.search_queries.push(format!("{} S{:02}E{:02}", title, season_num, episode_num));
                    metadata.search_queries.push(format!("{} S{}E{}", title, season_num, episode_num));
                    // Also add without colon (some trackers use this format)
                    metadata.search_queries.push(format!("{} S{:02}E{:02}", title.replace(":", ""), season_num, episode_num));
                }
                return Ok(metadata);
            }
        }

        // Fallback to OMDB
        if let Some(ref api_key) = self.omdb_api_key {
            if let Ok(mut metadata) = self.lookup_omdb(&base_imdb_id, api_key).await {
                // Add season/episode specific queries for series
                if let (Some(season_num), Some(episode_num)) = (season, episode) {
                    let title = &metadata.title;
                    metadata.search_queries.push(format!("{} S{:02}E{:02}", title, season_num, episode_num));
                    metadata.search_queries.push(format!("{} S{}E{}", title, season_num, episode_num));
                    metadata.search_queries.push(format!("{} S{:02}E{:02}", title.replace(":", ""), season_num, episode_num));
                }
                return Ok(metadata);
            }
        }

        // Direct title fallback
        if !imdb_id.starts_with("tt") || imdb_id.len() < 3 {
            return Ok(ContentMetadata {
                title: imdb_id.to_string(),
                year: None,
                content_type: content_type.to_string(),
                search_queries: vec![imdb_id.to_string()],
            });
        }

        Err(anyhow!("No metadata API configured and cannot parse ID: {}", imdb_id))
    }

    async fn lookup_tmdb(&self, imdb_id: &str, content_type: &str, api_key: &str) -> Result<ContentMetadata> {
        let url = format!(
            "{}/find/{}?api_key={}&external_source=imdb_id",
            TMDB_API_BASE, imdb_id, api_key
        );

        let response = self.client
            .get(&url)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("TMDB API error: {}", response.status()));
        }

        let data: serde_json::Value = response.json().await?;
        
        let results_key = if content_type == "movie" {
            "movie_results"
        } else {
            "tv_results"
        };

        if let Some(results) = data.get(results_key).and_then(|r| r.as_array()) {
            if let Some(first) = results.first() {
                let title = first
                    .get("title")
                    .or_else(|| first.get("name"))
                    .and_then(|t| t.as_str())
                    .unwrap_or(imdb_id);

                let year = first
                    .get("release_date")
                    .or_else(|| first.get("first_air_date"))
                    .and_then(|d| d.as_str())
                    .and_then(|d| d.split('-').next())
                    .map(|y| y.to_string());

                let original_title = first
                    .get("original_title")
                    .or_else(|| first.get("original_name"))
                    .and_then(|t| t.as_str());

                let mut queries = vec![title.to_string()];
                
                if let Some(orig) = original_title {
                    if orig != title {
                        queries.push(orig.to_string());
                    }
                }

                if let Some(ref y) = year {
                    queries.push(format!("{} {}", title, y));
                }

                return Ok(ContentMetadata {
                    title: title.to_string(),
                    year,
                    content_type: content_type.to_string(),
                    search_queries: queries,
                });
            }
        }

        Err(anyhow!("No results from TMDB for {}", imdb_id))
    }

    async fn lookup_omdb(&self, imdb_id: &str, api_key: &str) -> Result<ContentMetadata> {
        let url = format!(
            "{}?i={}&apikey={}",
            OMDB_API_BASE, imdb_id, api_key
        );

        let response = self.client
            .get(&url)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("OMDB API error: {}", response.status()));
        }

        let data: OMDBResult = response.json().await?;

        let title = data.title.ok_or_else(|| anyhow!("No title in OMDB response"))?;
        let content_type = data.content_type.unwrap_or_else(|| "movie".to_string());
        
        let mut queries = vec![title.clone()];
        if let Some(ref year) = data.year {
            queries.push(format!("{} {}", title, year));
        }

        Ok(ContentMetadata {
            title,
            year: data.year,
            content_type,
            search_queries: queries,
        })
    }

    pub async fn search_tmdb(&self, query: &str, content_type: &str, api_key: &str) -> Result<Vec<ContentMetadata>> {
        let endpoint = if content_type == "movie" {
            "search/movie"
        } else {
            "search/tv"
        };

        let url = format!(
            "{}/{}?api_key={}&query={}&page=1",
            TMDB_API_BASE, endpoint, api_key, urlencoding::encode(query)
        );

        let response = self.client
            .get(&url)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("TMDB API error: {}", response.status()));
        }

        let data: TMDBSearchResult = response.json().await?;
        
        let results: Vec<ContentMetadata> = data.results
            .into_iter()
            .take(5)
            .map(|item| {
                let title = item.title
                    .or(item.name)
                    .or(item.original_title)
                    .or(item.original_name)
                    .unwrap_or_default();

                let year = item.release_date
                    .or_else(|| item.first_air_date)
                    .map(|d| d.split('-').next().map(|y| y.to_string()).unwrap_or_default())
                    .filter(|y| !y.is_empty());

                let mut queries = vec![title.clone()];
                if let Some(ref y) = year {
                    queries.push(format!("{} {}", title, y));
                }

                ContentMetadata {
                    title,
                    year,
                    content_type: content_type.to_string(),
                    search_queries: queries,
                }
            })
            .filter(|m| !m.title.is_empty())
            .collect();

        Ok(results)
    }
}