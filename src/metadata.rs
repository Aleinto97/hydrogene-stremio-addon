use anyhow::{anyhow, Result};
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::cache::{is_cache_entry_fresh, CachedMetadata, MetadataCache};

const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";
const OMDB_API_BASE: &str = "http://www.omdbapi.com";
const ANILIST_API_BASE: &str = "https://graphql.anilist.co";

pub struct MetadataClient {
    client: Arc<Client>,
    tmdb_api_key: Option<String>,
    omdb_api_key: Option<String>,
    cache: Option<Arc<MetadataCache>>,
    memory_cache: Arc<RwLock<HashMap<String, CachedMetadata>>>,
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
            cache: None,
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn with_cache(mut self, cache: Arc<MetadataCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    pub async fn lookup_by_imdb(
        &self,
        imdb_id: &str,
        content_type: &str,
    ) -> Result<ContentMetadata> {
        let id_parts: Vec<&str> = imdb_id.split(':').collect();
        let (base_imdb_id, season, episode) = if id_parts.len() >= 3 {
            let base = id_parts[0].to_string();
            let season = id_parts[1].parse::<u32>().ok();
            let episode = id_parts[2].parse::<u32>().ok();
            (base, season, episode)
        } else {
            (imdb_id.to_string(), None, None)
        };

        let cache_key = imdb_id.to_string();

        if let Some(cached) = self.get_from_memory_cache(&cache_key).await {
            return Ok(Self::to_content_metadata(cached));
        }

        if let Some(ref cache) = self.cache {
            if let Some(cached) = cache.get(&cache_key).await? {
                self.store_in_memory_cache(&cache_key, &cached).await;
                return Ok(Self::to_content_metadata(cached));
            }
        }

        if imdb_id.starts_with("anilist:") {
            return self.lookup_anime(imdb_id, season, episode).await;
        }

        let mut metadata = if let Some(ref api_key) = self.tmdb_api_key {
            self.lookup_tmdb(&base_imdb_id, content_type, api_key)
                .await
                .ok()
        } else {
            None
        };

        if metadata.is_none() {
            if let Some(ref api_key) = self.omdb_api_key {
                metadata = self.lookup_omdb(&base_imdb_id, api_key).await.ok();
            }
        }

        let mut metadata = metadata.ok_or_else(|| anyhow!("No metadata found for {}", imdb_id))?;

        if let (Some(s), Some(e)) = (season, episode) {
            self.add_episode_queries(&mut metadata, s, e);
        }

        let cached = CachedMetadata {
            title: metadata.title.clone(),
            year: metadata.year.clone(),
            content_type: metadata.content_type.clone(),
            search_queries: metadata.search_queries.clone(),
            created_at: Utc::now(),
        };
        self.store_in_caches(&cache_key, &cached).await;

        Ok(metadata)
    }

    async fn lookup_anime(
        &self,
        anime_id: &str,
        _season: Option<u32>,
        episode: Option<u32>,
    ) -> Result<ContentMetadata> {
        let parts: Vec<&str> = anime_id.split(':').collect();
        let anime_num_id = parts
            .get(1)
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(|| anyhow!("Invalid anime ID"))?;

        let query = serde_json::json!({
            "query": r#"
                query ($id: Int) {
                    Media (id: $id, type: ANIME) {
                        title { english romaji native }
                        synonyms
                        startDate { year }
                    }
                }
            "#,
            "variables": { "id": anime_num_id }
        });

        let response = self
            .client
            .post(ANILIST_API_BASE)
            .header("Content-Type", "application/json")
            .json(&query)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("AniList API error: {}", response.status()));
        }

        let data: serde_json::Value = response.json().await?;
        let media = &data["data"]["Media"];

        let english_title = media["title"]["english"].as_str();
        let romaji_title = media["title"]["romaji"].as_str();
        let native_title = media["title"]["native"].as_str();
        let synonyms: Vec<String> = media["synonyms"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let year = media["startDate"]["year"].as_i64().map(|y| y.to_string());

        let primary_title = english_title
            .or(romaji_title)
            .or(native_title)
            .ok_or_else(|| anyhow!("No title found"))?;

        let mut queries = Vec::new();
        let year_str = year.as_deref();

        self.add_anime_title_variants(&mut queries, primary_title, year_str);

        if let Some(romaji) = romaji_title {
            if romaji != primary_title {
                self.add_anime_title_variants(&mut queries, romaji, year_str);
            }
        }

        if let Some(native) = native_title {
            if native != primary_title {
                self.add_anime_title_variants(&mut queries, native, None);
            }
        }

        for synonym in synonyms.iter().take(6) {
            self.add_anime_title_variants(&mut queries, synonym, year_str);
        }

        if let Some(ep) = episode {
            let ep_queries = self.build_anime_episode_queries(&queries, ep);
            queries.extend(ep_queries);
        }

        let cached = CachedMetadata {
            title: primary_title.to_string(),
            year: year.clone(),
            content_type: "series".to_string(),
            search_queries: queries.clone(),
            created_at: Utc::now(),
        };
        self.store_in_caches(anime_id, &cached).await;

        Ok(ContentMetadata {
            title: primary_title.to_string(),
            year,
            content_type: "series".to_string(),
            search_queries: queries,
        })
    }

    fn build_anime_episode_queries(&self, base_titles: &[String], episode: u32) -> Vec<String> {
        let mut queries = Vec::new();

        for title in base_titles {
            for candidate in [
                format!("{} {:02}", title, episode),
                format!("{} - {:02}", title, episode),
                format!("{} - {:02}v2", title, episode),
                format!("{} E{:02}", title, episode),
                format!("{} E{}", title, episode),
                format!("{} EP{:02}", title, episode),
                format!("{} EP{}", title, episode),
                format!("{} Episode {:02}", title, episode),
                format!("{} Episode {}", title, episode),
                format!("{} [{:02}]", title, episode),
                format!("{} [{:02}v2]", title, episode),
            ] {
                Self::push_unique_query(&mut queries, candidate);
            }

            if episode < 10 {
                for candidate in [
                    format!("{} {}", title, episode),
                    format!("{} - {}", title, episode),
                    format!("{} - {}v2", title, episode),
                ] {
                    Self::push_unique_query(&mut queries, candidate);
                }
            }
        }

        queries
    }

    fn add_anime_title_variants(&self, queries: &mut Vec<String>, title: &str, year: Option<&str>) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }

        Self::push_unique_query(queries, title.to_string());

        if let Some(year) = year {
            Self::push_unique_query(queries, format!("{} {}", title, year));
        }

        let no_colon = title.replace(':', "");
        if no_colon != title {
            Self::push_unique_query(queries, no_colon.clone());
            if let Some(year) = year {
                Self::push_unique_query(queries, format!("{} {}", no_colon, year));
            }
        }

        if let Some((head, _)) = title.split_once(':') {
            let head = head.trim();
            if head.split_whitespace().count() >= 2 {
                Self::push_unique_query(queries, head.to_string());
                if let Some(year) = year {
                    Self::push_unique_query(queries, format!("{} {}", head, year));
                }
            }
        }
    }

    fn push_unique_query(queries: &mut Vec<String>, candidate: String) {
        let candidate = candidate.trim().to_string();
        if candidate.is_empty() {
            return;
        }

        if queries
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&candidate))
        {
            return;
        }

        queries.push(candidate);
    }

    fn add_episode_queries(&self, metadata: &mut ContentMetadata, season: u32, episode: u32) {
        let title = &metadata.title;
        let title_no_colon = title.replace(":", "");

        let episode_queries = vec![
            format!("{} S{:02}E{:02}", title, season, episode),
            format!("{} S{}E{}", title, season, episode),
            format!("{} S{:02}E{:02}", title_no_colon, season, episode),
            format!("{} {}x{:02}", title, season, episode),
            format!("{} Season {} Episode {}", title, season, episode),
            format!("{} S{:02}", title, season),
        ];

        metadata.search_queries.extend(episode_queries);

        if let Some(ref year) = metadata.year {
            metadata
                .search_queries
                .push(format!("{} {} S{:02}E{:02}", title, year, season, episode));
        }
    }

    async fn lookup_tmdb(
        &self,
        imdb_id: &str,
        content_type: &str,
        api_key: &str,
    ) -> Result<ContentMetadata> {
        let url = format!(
            "{}/find/{}?api_key={}&external_source=imdb_id",
            TMDB_API_BASE, imdb_id, api_key
        );

        let response = self.client.get(&url).send().await?;

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
        let url = format!("{}?i={}&apikey={}", OMDB_API_BASE, imdb_id, api_key);

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("OMDB API error: {}", response.status()));
        }

        let data: OMDBResult = response.json().await?;

        let title = data
            .title
            .ok_or_else(|| anyhow!("No title in OMDB response"))?;
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

    async fn get_from_memory_cache(&self, cache_key: &str) -> Option<CachedMetadata> {
        let cached = {
            let memory_cache = self.memory_cache.read().await;
            memory_cache.get(cache_key).cloned()
        };

        match cached {
            Some(cached) if is_cache_entry_fresh(&cached) => {
                tracing::debug!("Memory cache HIT for metadata: {}", cache_key);
                Some(cached)
            }
            Some(_) => {
                tracing::debug!("Memory cache EXPIRED for metadata: {}", cache_key);
                let mut memory_cache = self.memory_cache.write().await;
                memory_cache.remove(cache_key);
                None
            }
            None => {
                tracing::debug!("Memory cache MISS for metadata: {}", cache_key);
                None
            }
        }
    }

    async fn store_in_caches(&self, cache_key: &str, cached: &CachedMetadata) {
        self.store_in_memory_cache(cache_key, cached).await;

        if let Some(ref cache) = self.cache {
            let _ = cache.set(cache_key, cached).await;
        }
    }

    async fn store_in_memory_cache(&self, cache_key: &str, cached: &CachedMetadata) {
        let mut memory_cache = self.memory_cache.write().await;
        memory_cache.insert(cache_key.to_string(), cached.clone());
    }

    fn to_content_metadata(cached: CachedMetadata) -> ContentMetadata {
        ContentMetadata {
            title: cached.title,
            year: cached.year,
            content_type: cached.content_type,
            search_queries: cached.search_queries,
        }
    }
}
