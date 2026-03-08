use reqwest::Client;
use serde::{Deserialize, Serialize};
use anyhow::{Result, anyhow};

const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";
const OMDB_API_BASE: &str = "http://www.omdbapi.com";
const KITSU_API_BASE: &str = "https://kitsu.io/api/edge";

pub struct MetadataClient {
    client: Client,
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

#[derive(Debug, Deserialize)]
struct KitsuAnimeResponse {
    data: KitsuAnimeData,
}

#[derive(Debug, Deserialize)]
struct KitsuAnimeData {
    attributes: KitsuAnimeAttributes,
}

#[derive(Debug, Deserialize)]
struct KitsuAnimeAttributes {
    #[serde(rename = "canonicalTitle")]
    canonical_title: String,
    #[serde(rename = "titles")]
    titles: Option<KitsuTitles>,
}

#[derive(Debug, Deserialize)]
struct KitsuTitles {
    #[serde(rename = "en")]
    en: Option<String>,
    #[serde(rename = "en_jp")]
    en_jp: Option<String>,
    #[serde(rename = "ja_jp")]
    ja_jp: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ContentMetadata {
    pub title: String,
    pub year: Option<String>,
    pub content_type: String,
    pub search_queries: Vec<String>,
}

impl MetadataClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        let tmdb_api_key = std::env::var("TMDB_API_KEY").ok();
        let omdb_api_key = std::env::var("OMDB_API_KEY").ok();

        Ok(Self {
            client,
            tmdb_api_key,
            omdb_api_key,
        })
    }

    pub async fn lookup_by_imdb(&self, imdb_id: &str, content_type: &str) -> Result<ContentMetadata> {
        // Handle Kitsu anime IDs first
        if imdb_id.starts_with("kitsu:") {
            let kitsu_id = imdb_id.replace("kitsu:", "");
            tracing::info!("Detected Kitsu ID: {}, attempting lookup", kitsu_id);
            
            // Try Kitsu API first
            match self.lookup_kitsu(&kitsu_id).await {
                Ok(metadata) => {
                    tracing::info!("Kitsu lookup successful with {} queries", metadata.search_queries.len());
                    if !metadata.search_queries.is_empty() {
                        return Ok(metadata);
                    }
                }
                Err(e) => {
                    tracing::warn!("Kitsu lookup failed: {}, will try alternatives", e);
                }
            }
            
            // Fallback 1: Try TMDB with the kitsu_id (some anime have IMDB entries)
            if let Some(ref api_key) = self.tmdb_api_key {
                // Try searching TMDB directly with "anime [id]"
                let search_query = format!("anime {}", kitsu_id);
                if let Ok(results) = self.search_tmdb(&search_query, "tv", api_key).await {
                    if let Some(first) = results.first() {
                        tracing::info!("Found anime via TMDB search: {}", first.title);
                        return Ok(first.clone());
                    }
                }
            }
            
            // Fallback 2: Use ID with common anime terms
            tracing::info!("Using Kitsu ID with generic search terms as fallback");
            return Ok(ContentMetadata {
                title: format!("Kitsu {}", kitsu_id),
                year: None,
                content_type: "series".to_string(),
                search_queries: vec![
                    kitsu_id.clone(),
                    format!("{} anime", kitsu_id),
                ],
            });
        }

        // Try TMDB first
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

        // If no API keys configured, try to parse IMDB ID patterns
        if !imdb_id.starts_with("tt") || imdb_id.len() < 3 {
            return Ok(ContentMetadata {
                title: imdb_id.to_string(),
                year: None,
                content_type: content_type.to_string(),
                search_queries: vec![imdb_id.to_string()],
            });
        }

        Err(anyhow!("No metadata API configured and cannot parse IMDB ID: {}", imdb_id))
    }

    async fn lookup_kitsu(&self, kitsu_id: &str) -> Result<ContentMetadata> {
        let url = format!("{}/anime/{}", KITSU_API_BASE, kitsu_id);
        
        tracing::info!("Kitsu API lookup for ID: {} - URL: {}", kitsu_id, url);
        
        let response = match self.client
            .get(&url)
            .header("Accept", "application/vnd.api+json")
            .header("Content-Type", "application/vnd.api+json")
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!("Kitsu API request failed for {}: {}", kitsu_id, e);
                return Err(anyhow!("Kitsu API request failed: {}", e));
            }
        };

        if !response.status().is_success() {
            tracing::error!("Kitsu API returned status: {} for ID {}", response.status(), kitsu_id);
            return Err(anyhow!("Kitsu API error: {}", response.status()));
        }

        let data: KitsuAnimeResponse = match response.json().await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("Kitsu API JSON parse failed for {}: {}", kitsu_id, e);
                return Err(anyhow!("Kitsu JSON parse error: {}", e));
            }
        };
        
        let attrs = data.data.attributes;
        
        let canonical = attrs.canonical_title.clone();
        let en = attrs.titles.as_ref().and_then(|t| t.en.clone());
        let en_jp = attrs.titles.as_ref().and_then(|t| t.en_jp.clone());
        let ja_jp = attrs.titles.as_ref().and_then(|t| t.ja_jp.clone());
        
        tracing::info!("Kitsu API success for {}: canonical={}, en={:?}, en_jp={:?}", 
                     kitsu_id, canonical, en, en_jp);
        
        // Build search queries with all available titles
        let mut queries = vec![];
        
        // Prefer English title, fallback to others
        let primary_title = en.clone()
            .or_else(|| Some(canonical.clone()))
            .unwrap_or_default();
        
        if !primary_title.is_empty() {
            queries.push(primary_title.clone());
        }
        
        if let Some(ref en_jp) = en_jp {
            if !queries.contains(en_jp) && !en_jp.is_empty() {
                queries.push(en_jp.clone());
            }
        }
        
        if let Some(ref ja) = ja_jp {
            if !queries.contains(ja) && !ja.is_empty() {
                queries.push(ja.clone());
            }
        }
        
        // Add canonical if not already in list
        if !queries.contains(&canonical) && !canonical.is_empty() {
            queries.push(canonical);
        }

        Ok(ContentMetadata {
            title: primary_title,
            year: None,
            content_type: "series".to_string(),
            search_queries: queries,
        })
    }

    async fn lookup_tmdb(&self, imdb_id: &str, content_type: &str, api_key: &str) -> Result<ContentMetadata> {
        // TMDB supports external ID lookup
        let endpoint = if content_type == "movie" {
            "movie"
        } else {
            "tv"
        };

        let _url = format!(
            "{}/{}/external_ids?api_key={}&external_source=imdb_id",
            TMDB_API_BASE, endpoint, api_key
        );

        // Actually, let's use find by external ID
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
        
        // Extract from movie_results or tv_results
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
                    .map(|d| d.split('-').next().map(|y| y.to_string()))
                    .flatten();

                let original_title = first
                    .get("original_title")
                    .or_else(|| first.get("original_name"))
                    .and_then(|t| t.as_str());

                // Build search queries with variations
                let mut queries = vec![title.to_string()];
                
                if let Some(orig) = original_title {
                    if orig != title {
                        queries.push(orig.to_string());
                    }
                }

                // Add year to queries if available
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

    // Direct search without IMDB lookup
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
            .take(5) // Limit to top 5 results
            .map(|item| {
                let title = item.title
                    .or(item.name)
                    .or(item.original_title)
                    .or(item.original_name)
                    .unwrap_or_default();

                let year = item.release_date
                    .or_else(|| item.first_air_date)
                    .map(|d| {
                        d.split('-').next().map(|y| y.to_string())
                    })
                    .flatten();

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