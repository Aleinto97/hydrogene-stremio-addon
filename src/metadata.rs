use reqwest::Client;
use serde::{Deserialize, Serialize};
use anyhow::{Result, anyhow};
use std::sync::Arc;

const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";
const OMDB_API_BASE: &str = "http://www.omdbapi.com";
const ANILIST_API_BASE: &str = "https://graphql.anilist.co";
const KITSU_PROXY_BASE: &str = "https://anime-kitsu.strem.fun";

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

// Kitsu Proxy CDN Response (Community Stremio Addon)
#[derive(Debug, Deserialize)]
struct KitsuProxyResponse {
    meta: KitsuProxyMeta,
}

#[derive(Debug, Deserialize)]
struct KitsuProxyMeta {
    name: String,
    #[serde(rename = "type")]
    content_type: String,
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

    /// Resolve anime ID (kitsu: or anilist:) to title using CDN/GraphQL
    pub async fn resolve_anime_title(&self, stremio_id: &str) -> Option<String> {
        let parts: Vec<&str> = stremio_id.split(':').collect();
        
        eprintln!("RESOLVE: Called with stremio_id: {}", stremio_id);
        eprintln!("RESOLVE: Parts: {:?}", parts);
        
        match parts.as_slice() {
            // --- CASE: KITSU via Community CDN ---
            ["kitsu", id] | ["kitsu", id, _] => {
                let url = format!("{}/meta/anime/kitsu:{}.json", KITSU_PROXY_BASE, id);
                eprintln!("RESOLVE: Kitsu URL: {}", url);
                
                match self.client.get(&url).send().await {
                    Ok(res) if res.status().is_success() => {
                        eprintln!("RESOLVE: Kitsu response OK");
                        match res.json::<KitsuProxyResponse>().await {
                            Ok(data) => {
                                eprintln!("RESOLVE: Kitsu SUCCESS -> '{}'", data.meta.name);
                                return Some(data.meta.name);
                            }
                            Err(e) => {
                                eprintln!("RESOLVE: Kitsu JSON parse error: {}", e);
                            }
                        }
                    }
                    Ok(res) => {
                        eprintln!("RESOLVE: Kitsu bad status: {}", res.status());
                    }
                    Err(e) => {
                        eprintln!("RESOLVE: Kitsu request error: {}", e);
                    }
                }
            }
            
            // --- CASE: ANILIST via Minimal GraphQL ---
            ["anilist", id] | ["anilist", id, _] => {
                let anime_id = id.parse::<i32>().unwrap_or(0);
                eprintln!("RESOLVE: AniList ID parsed: {}", anime_id);
                
                if anime_id == 0 {
                    eprintln!("RESOLVE: AniList ID is 0, returning None");
                    return None;
                }
                
                let query = serde_json::json!({
                    "query": "query ($id: Int) { Media (id: $id, type: ANIME) { title { english romaji } } }",
                    "variables": { "id": anime_id }
                });
                
                eprintln!("RESOLVE: AniList posting to {}", ANILIST_API_BASE);
                
                match self.client
                    .post(ANILIST_API_BASE)
                    .header("Content-Type", "application/json")
                    .json(&query)
                    .send()
                    .await
                {
                    Ok(res) if res.status().is_success() => {
                        eprintln!("RESOLVE: AniList response OK");
                        match res.json::<serde_json::Value>().await {
                            Ok(data) => {
                                eprintln!("RESOLVE: AniList JSON parsed: {:?}", data["data"]["Media"]["title"]);
                                // Try English title first, fallback to romaji
                                let title = data["data"]["Media"]["title"]["english"]
                                    .as_str()
                                    .or_else(|| data["data"]["Media"]["title"]["romaji"].as_str());
                                
                                if let Some(t) = title {
                                    eprintln!("RESOLVE: AniList SUCCESS -> '{}'", t);
                                    return Some(t.to_string());
                                } else {
                                    eprintln!("RESOLVE: AniList no title found in response");
                                }
                            }
                            Err(e) => {
                                eprintln!("RESOLVE: AniList JSON parse error: {}", e);
                            }
                        }
                    }
                    Ok(res) => {
                        eprintln!("RESOLVE: AniList bad status: {}", res.status());
                    }
                    Err(e) => {
                        eprintln!("RESOLVE: AniList request error: {}", e);
                    }
                }
            }
            
            _ => {
                eprintln!("RESOLVE: Not a recognized anime ID format: {}", stremio_id);
            }
        }
        
        eprintln!("RESOLVE: Returning None for {}", stremio_id);
        None
    }

    pub async fn lookup_by_imdb(&self, imdb_id: &str, content_type: &str) -> Result<ContentMetadata> {
        // Handle anime IDs (kitsu: or anilist:) via CDN/GraphQL
        if imdb_id.starts_with("kitsu:") || imdb_id.starts_with("anilist:") {
            tracing::info!("Resolving anime ID via CDN/GraphQL: {}", imdb_id);
            
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
            let fallback_id = imdb_id
                .replace("kitsu:", "")
                .replace("anilist:", "");
            
            return Ok(ContentMetadata {
                title: format!("Anime {}", fallback_id),
                year: None,
                content_type: "series".to_string(),
                search_queries: vec![fallback_id],
            });
        }

        // Try TMDB for movies/series
        if let Some(ref api_key) = self.tmdb_api_key {
            if let Ok(metadata) = self.lookup_tmdb(imdb_id, content_type, api_key).await {
                return Ok(metadata);
            }
        }

        // Fallback to OMDB
        if let Some(ref api_key) = self.omdb_api_key {
            if let Ok(metadata) = self.lookup_omdb(imdb_id, api_key).await {
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
                    .and_then(|d| d.split('-').next())
                    .map(|y| y.to_string());

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