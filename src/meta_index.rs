use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::hydra::{HydraId, HydraKind, HydraSource};
use crate::metadata::ContentMetadata;

const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedEpisode {
    pub season: u32,
    pub episode: u32,
    pub title: String,
    pub released_at: Option<String>,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaTrailer {
    pub source: String,
    #[serde(rename = "type")]
    pub trailer_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedContent {
    pub hydra_id: String,
    pub kind: String,
    pub content_type: String,
    pub primary_source: String,
    pub primary_id: String,
    pub title: String,
    pub year: Option<String>,
    pub description: Option<String>,
    pub poster: Option<String>,
    pub background: Option<String>,
    pub logo: Option<String>,
    pub runtime: Option<String>,
    pub genres: Vec<String>,
    pub popularity: f64,
    pub is_anime: bool,
    pub aliases: Vec<String>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<i64>,
    pub anidb_id: Option<i32>,
    pub released: Option<String>,
    pub imdb_rating: Option<String>,
    pub director: Vec<String>,
    pub cast: Vec<String>,
    pub trailers: Vec<MetaTrailer>,
    pub episodes: Vec<IndexedEpisode>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogMeta {
    pub id: String,
    #[serde(rename = "type")]
    pub content_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "releaseInfo")]
    pub release_info: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "imdbRating")]
    pub imdb_rating: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub director: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub cast: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub trailers: Vec<MetaTrailer>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetaVideo {
    pub id: String,
    pub title: String,
    pub season: u32,
    pub episode: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetaItem {
    pub id: String,
    #[serde(rename = "type")]
    pub content_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "releaseInfo")]
    pub release_info: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "imdbRating")]
    pub imdb_rating: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub director: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub cast: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub trailers: Vec<MetaTrailer>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub videos: Vec<MetaVideo>,
}

#[derive(Clone)]
pub struct MetadataIndex {
    client: Arc<Client>,
    tmdb_api_key: Option<String>,
    imdb_akas_url: Option<String>,
    imdb_basics_url: Option<String>,
    anidb_titles_source: Option<String>,
    pool: Option<PgPool>,
    items: Arc<RwLock<HashMap<String, IndexedContent>>>,
}

#[derive(Debug, Deserialize)]
struct TmdbListResponse {
    #[serde(default)]
    results: Vec<TmdbListItem>,
}

#[derive(Debug, Deserialize)]
struct TmdbListItem {
    id: i64,
    title: Option<String>,
    name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    #[serde(default)]
    genre_ids: Vec<i64>,
    #[serde(default)]
    popularity: f64,
    release_date: Option<String>,
    first_air_date: Option<String>,
    original_language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbGenreResponse {
    genres: Vec<TmdbGenre>,
}

#[derive(Debug, Deserialize, Clone)]
struct TmdbGenre {
    id: i64,
    name: String,
}

#[derive(Debug, Deserialize)]
struct TmdbMovieDetails {
    id: i64,
    title: String,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    release_date: Option<String>,
    runtime: Option<u32>,
    #[serde(default)]
    genres: Vec<TmdbGenre>,
    external_ids: Option<TmdbExternalIds>,
    #[serde(default)]
    popularity: f64,
    #[serde(default)]
    vote_average: f64,
    credits: Option<TmdbCredits>,
    images: Option<TmdbImages>,
    videos: Option<TmdbVideos>,
}

#[derive(Debug, Deserialize)]
struct TmdbTvDetails {
    id: i64,
    name: String,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    first_air_date: Option<String>,
    episode_run_time: Option<Vec<u32>>,
    #[serde(default)]
    genres: Vec<TmdbGenre>,
    external_ids: Option<TmdbExternalIds>,
    #[serde(default)]
    popularity: f64,
    #[serde(default)]
    seasons: Vec<TmdbSeasonSummary>,
    #[serde(default)]
    number_of_episodes: u32,
    original_language: Option<String>,
    #[serde(default)]
    vote_average: f64,
    #[serde(default)]
    created_by: Vec<TmdbNamedPerson>,
    aggregate_credits: Option<TmdbAggregateCredits>,
    images: Option<TmdbImages>,
    videos: Option<TmdbVideos>,
}

#[derive(Debug, Deserialize)]
struct TmdbExternalIds {
    imdb_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbSeasonSummary {
    season_number: u32,
}

#[derive(Debug, Deserialize)]
struct TmdbSeasonDetails {
    #[serde(default)]
    episodes: Vec<TmdbEpisode>,
}

#[derive(Debug, Deserialize)]
struct TmdbEpisode {
    episode_number: u32,
    name: String,
    air_date: Option<String>,
    still_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbCredits {
    #[serde(default)]
    cast: Vec<TmdbCastMember>,
    #[serde(default)]
    crew: Vec<TmdbCrewMember>,
}

#[derive(Debug, Deserialize)]
struct TmdbAggregateCredits {
    #[serde(default)]
    cast: Vec<TmdbCastMember>,
    #[serde(default)]
    crew: Vec<TmdbCrewMember>,
}

#[derive(Debug, Deserialize)]
struct TmdbCastMember {
    name: String,
}

#[derive(Debug, Deserialize)]
struct TmdbCrewMember {
    name: String,
    job: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbNamedPerson {
    name: String,
}

#[derive(Debug, Deserialize)]
struct TmdbImages {
    #[serde(default)]
    logos: Vec<TmdbImageAsset>,
}

#[derive(Debug, Deserialize)]
struct TmdbImageAsset {
    file_path: String,
    #[serde(default)]
    iso_639_1: Option<String>,
    #[serde(default)]
    vote_average: f64,
    #[serde(default)]
    file_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbVideos {
    #[serde(default)]
    results: Vec<TmdbVideoAsset>,
}

#[derive(Debug, Deserialize)]
struct TmdbVideoAsset {
    key: String,
    site: String,
    #[serde(rename = "type")]
    video_type: String,
    #[serde(default)]
    official: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct IndexedContentExtra {
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    released: Option<String>,
    #[serde(default)]
    imdb_rating: Option<String>,
    #[serde(default)]
    director: Vec<String>,
    #[serde(default)]
    cast: Vec<String>,
    #[serde(default)]
    trailers: Vec<MetaTrailer>,
}

impl MetadataIndex {
    pub async fn new(client: Arc<Client>) -> Result<Self> {
        let database_url = std::env::var("DATABASE_URL").ok();
        let pool = if let Some(database_url) = database_url {
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .min_connections(1)
                .connect(&database_url)
                .await?;
            Self::ensure_tables(&pool).await?;
            Some(pool)
        } else {
            None
        };

        let index = Self {
            client,
            tmdb_api_key: std::env::var("TMDB_API_KEY").ok(),
            imdb_akas_url: std::env::var("IMDB_AKAS_URL").ok(),
            imdb_basics_url: std::env::var("IMDB_BASICS_URL").ok(),
            anidb_titles_source: std::env::var("ANIDB_TITLES_SOURCE").ok(),
            pool,
            items: Arc::new(RwLock::new(HashMap::new())),
        };

        index.load_from_store().await?;
        Ok(index)
    }

    pub async fn bootstrap(&self) -> Result<()> {
        if !self.items.read().await.is_empty() {
            return Ok(());
        }

        tracing::info!("Bootstrapping metadata index");
        self.seed_featured_content().await?;
        self.sync_default_catalogs().await?;
        self.import_anidb_titles().await?;
        self.import_imdb_aliases().await?;
        Ok(())
    }

    pub fn spawn_refresh_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60 * 6));
            loop {
                interval.tick().await;
                if let Err(error) = self.sync_default_catalogs().await {
                    tracing::warn!("Metadata refresh failed: {}", error);
                }
                if let Err(error) = self.import_anidb_titles().await {
                    tracing::warn!("AniDB alias refresh failed: {}", error);
                }
                if let Err(error) = self.import_imdb_aliases().await {
                    tracing::warn!("IMDb alias refresh failed: {}", error);
                }
            }
        });
    }

    pub async fn get_meta(&self, id: &str, content_type: &str) -> Result<Option<MetaItem>> {
        if let Some(hydra) = HydraId::parse(id) {
            self.ensure_hydra_loaded(&hydra).await?;
            let item = {
                let items = self.items.read().await;
                items.get(&hydra.base_key()).cloned()
            };
            return Ok(item.map(|item| self.to_meta_item(&item)));
        }

        if let Some(tmdb_id) = id
            .strip_prefix("tmdb:")
            .and_then(|value| value.parse::<i64>().ok())
        {
            let hydra_kind = if content_type == "movie" {
                HydraKind::Movie
            } else {
                HydraKind::Series
            };
            let hydra = HydraId::new(hydra_kind, HydraSource::Tmdb, tmdb_id.to_string());
            self.ensure_hydra_loaded(&hydra).await?;
            let item = {
                let items = self.items.read().await;
                items.get(&hydra.base_key()).cloned()
            };
            return Ok(item.map(|item| self.to_meta_item(&item)));
        }

        if id.starts_with("tt") {
            self.search_and_store(id, content_type).await?;
            let item = {
                let items = self.items.read().await;
                items
                    .values()
                    .find(|item| item.imdb_id.as_deref() == Some(id))
                    .cloned()
            };
            return Ok(item.map(|item| self.to_meta_item(&item)));
        }

        Ok(None)
    }

    pub async fn catalog(
        &self,
        content_type: &str,
        catalog_id: &str,
        search: Option<&str>,
        genre: Option<&str>,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<CatalogMeta>> {
        if let Some(search) = search.filter(|query| !query.trim().is_empty()) {
            self.search_and_store(search, content_type).await?;
        }

        let items = self.items.read().await;
        let mut metas: Vec<IndexedContent> = items
            .values()
            .filter(|item| item.content_type == content_type)
            .filter(|item| Self::catalog_matches(item, catalog_id))
            .filter(|item| {
                genre
                    .map(|expected| {
                        item.genres
                            .iter()
                            .any(|genre_name| genre_name.eq_ignore_ascii_case(expected))
                    })
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        if let Some(search) = search {
            let query = search.to_lowercase();
            metas.sort_by(|a, b| {
                let a_score = Self::search_score(a, &query);
                let b_score = Self::search_score(b, &query);
                b_score
                    .cmp(&a_score)
                    .then_with(|| b.popularity.total_cmp(&a.popularity))
            });
        } else {
            metas.sort_by(|a, b| b.popularity.total_cmp(&a.popularity));
        }

        Ok(metas
            .into_iter()
            .skip(skip)
            .take(limit)
            .map(|item| CatalogMeta {
                id: item.hydra_id.clone(),
                content_type: item.content_type.clone(),
                name: item.title.clone(),
                poster: item.poster.clone(),
                logo: item.logo.clone(),
                background: item.background.clone(),
                description: item.description.clone(),
                genres: item.genres.clone(),
                release_info: item.year.clone(),
                imdb_rating: item.imdb_rating.clone(),
                director: item.director.clone(),
                cast: item.cast.clone(),
                trailers: item.trailers.clone(),
            })
            .collect())
    }

    pub async fn resolve_stream_metadata(&self, id: &str) -> Result<Option<ContentMetadata>> {
        let Some(hydra) = HydraId::parse(id) else {
            return Ok(None);
        };

        self.ensure_hydra_loaded(&hydra).await?;
        let item = {
            let items = self.items.read().await;
            items.get(&hydra.base_key()).cloned()
        };

        Ok(item.map(|item| {
            let queries = self.build_stream_queries(&item, hydra.season, hydra.episode);
            ContentMetadata {
                title: item.title,
                year: item.year,
                content_type: item.content_type,
                search_queries: queries,
            }
        }))
    }

    async fn ensure_hydra_loaded(&self, hydra: &HydraId) -> Result<()> {
        if let Some(existing) = self.items.read().await.get(&hydra.base_key()).cloned() {
            let missing_rich_meta = existing.logo.is_none()
                && existing.imdb_rating.is_none()
                && existing.director.is_empty()
                && existing.cast.is_empty()
                && existing.trailers.is_empty();
            let needs_tmdb_hydration = matches!(hydra.source, HydraSource::Tmdb)
                && ((hydra.kind == HydraKind::Movie
                    && (existing.imdb_id.is_none() || missing_rich_meta))
                    || (hydra.kind != HydraKind::Movie
                        && (existing.episodes.is_empty()
                            || missing_rich_meta
                            || existing.released.is_none())));
            if !needs_tmdb_hydration {
                return Ok(());
            }
        }

        match hydra.source {
            HydraSource::Tmdb => {
                self.hydrate_tmdb_id(hydra.kind, hydra.primary_id.parse::<i64>()?)
                    .await?;
            }
            HydraSource::Tt => {
                self.search_and_store(&hydra.primary_id, hydra.kind.as_str())
                    .await?;
            }
            HydraSource::Anidb => {
                self.search_and_store(&hydra.primary_id, "series").await?;
            }
        }

        Ok(())
    }

    async fn sync_default_catalogs(&self) -> Result<()> {
        let Some(api_key) = self.tmdb_api_key.as_deref() else {
            tracing::warn!(
                "TMDB_API_KEY not set, metadata index bootstrap limited to featured seeds"
            );
            return Ok(());
        };

        let genre_map = self.fetch_tmdb_genres(api_key).await?;
        let requests = [
            ("movie", "movie/popular"),
            ("movie", "trending/movie/week"),
            ("movie", "movie/upcoming"),
            ("series", "tv/popular"),
            ("series", "trending/tv/week"),
            ("series", "tv/on_the_air"),
        ];

        for (kind, path) in requests {
            let url = format!("{}/{}?api_key={}", TMDB_API_BASE, path, api_key);
            let payload: TmdbListResponse = self.client.get(&url).send().await?.json().await?;
            for item in payload.results {
                self.upsert_tmdb_summary(kind, &item, &genre_map).await?;
            }
        }

        let anime_url = format!(
            "{}/discover/tv?api_key={}&with_genres=16&with_original_language=ja&sort_by=popularity.desc",
            TMDB_API_BASE, api_key
        );
        let anime_payload: TmdbListResponse =
            self.client.get(&anime_url).send().await?.json().await?;
        for item in anime_payload.results {
            self.upsert_tmdb_summary("anime", &item, &genre_map).await?;
        }

        Ok(())
    }

    async fn seed_featured_content(&self) -> Result<()> {
        let featured = [
            ("Attack on Titan", HydraKind::Anime, Some("2013")),
            ("Death Note", HydraKind::Anime, Some("2006")),
            ("One Piece", HydraKind::Anime, Some("1999")),
            ("Breaking Bad", HydraKind::Series, Some("2008")),
            ("Inception", HydraKind::Movie, Some("2010")),
            ("Avatar The Last Airbender", HydraKind::Series, Some("2005")),
            ("Avatar The Last Airbender", HydraKind::Series, Some("2024")),
            ("The Crow", HydraKind::Movie, Some("1994")),
            ("The Crow", HydraKind::Movie, Some("2024")),
        ];

        for (title, kind, year) in featured {
            let _ = self.seed_from_tmdb_search(title, kind, year).await;
        }

        Ok(())
    }

    async fn seed_from_tmdb_search(
        &self,
        query: &str,
        kind: HydraKind,
        expected_year: Option<&str>,
    ) -> Result<()> {
        let Some(api_key) = self.tmdb_api_key.as_deref() else {
            return Ok(());
        };

        let endpoint = match kind {
            HydraKind::Movie => "search/movie",
            HydraKind::Series | HydraKind::Anime => "search/tv",
        };
        let url = format!(
            "{}/{}?api_key={}&query={}",
            TMDB_API_BASE,
            endpoint,
            api_key,
            urlencoding::encode(query)
        );
        let payload: TmdbListResponse = self.client.get(&url).send().await?.json().await?;
        let candidate = payload.results.into_iter().find(|item| {
            let year = item
                .release_date
                .as_ref()
                .or(item.first_air_date.as_ref())
                .and_then(|value| value.split('-').next())
                .unwrap_or_default();
            expected_year
                .map(|expected| expected == year)
                .unwrap_or(true)
        });

        if let Some(candidate) = candidate {
            let genre_map = self.fetch_tmdb_genres(api_key).await?;
            let kind_str = if kind == HydraKind::Anime {
                "anime"
            } else {
                kind.as_str()
            };
            self.upsert_tmdb_summary(kind_str, &candidate, &genre_map)
                .await?;
        }

        Ok(())
    }

    async fn upsert_tmdb_summary(
        &self,
        kind: &str,
        item: &TmdbListItem,
        genre_map: &HashMap<i64, String>,
    ) -> Result<()> {
        let title = item
            .title
            .as_ref()
            .or(item.name.as_ref())
            .cloned()
            .ok_or_else(|| anyhow!("TMDB summary missing title"))?;
        let year = item
            .release_date
            .as_ref()
            .or(item.first_air_date.as_ref())
            .and_then(|value| value.split('-').next())
            .map(str::to_string);
        let genres = item
            .genre_ids
            .iter()
            .filter_map(|genre_id| genre_map.get(genre_id).cloned())
            .collect::<Vec<_>>();
        let is_anime = kind == "anime"
            || (item.original_language.as_deref() == Some("ja")
                && genres
                    .iter()
                    .any(|genre| genre.eq_ignore_ascii_case("Animation")));
        let hydra_kind = if kind == "movie" {
            HydraKind::Movie
        } else if is_anime {
            HydraKind::Anime
        } else {
            HydraKind::Series
        };
        let hydra_id = HydraId::new(hydra_kind, HydraSource::Tmdb, item.id.to_string()).to_string();

        let mut aliases = vec![title.clone()];
        if let Some(pack) = featured_alias_pack(&title) {
            aliases.extend(pack.iter().map(|alias| alias.to_string()));
        }
        aliases.sort();
        aliases.dedup();

        let content = IndexedContent {
            hydra_id: hydra_id.clone(),
            kind: hydra_kind.as_str().to_string(),
            content_type: if hydra_kind == HydraKind::Movie {
                "movie".to_string()
            } else {
                "series".to_string()
            },
            primary_source: "tmdb".to_string(),
            primary_id: item.id.to_string(),
            title,
            year,
            description: item.overview.clone(),
            poster: image_url(item.poster_path.as_deref(), "w500"),
            background: image_url(item.backdrop_path.as_deref(), "w1280"),
            logo: None,
            runtime: None,
            genres,
            popularity: item.popularity,
            is_anime,
            aliases,
            imdb_id: None,
            tmdb_id: Some(item.id),
            anidb_id: None,
            released: item
                .release_date
                .as_ref()
                .or(item.first_air_date.as_ref())
                .and_then(|value| iso_datetime(value)),
            imdb_rating: None,
            director: Vec::new(),
            cast: Vec::new(),
            trailers: Vec::new(),
            episodes: Vec::new(),
            updated_at: Utc::now(),
        };

        self.store_content(content).await
    }

    async fn hydrate_tmdb_id(&self, kind: HydraKind, tmdb_id: i64) -> Result<()> {
        let Some(api_key) = self.tmdb_api_key.as_deref() else {
            return Ok(());
        };

        match kind {
            HydraKind::Movie => {
                let url = format!(
                    "{}/movie/{}?api_key={}&append_to_response=external_ids",
                    TMDB_API_BASE, tmdb_id, api_key
                );
                let details: TmdbMovieDetails = self.client.get(&url).send().await?.json().await?;
                let hydra = HydraId::new(HydraKind::Movie, HydraSource::Tmdb, tmdb_id.to_string());
                let content = IndexedContent {
                    hydra_id: hydra.to_string(),
                    kind: HydraKind::Movie.as_str().to_string(),
                    content_type: "movie".to_string(),
                    primary_source: "tmdb".to_string(),
                    primary_id: tmdb_id.to_string(),
                    title: details.title.clone(),
                    year: details
                        .release_date
                        .as_deref()
                        .and_then(|value| value.split('-').next())
                        .map(str::to_string),
                    description: details.overview.clone(),
                    poster: image_url(details.poster_path.as_deref(), "w500"),
                    background: image_url(details.backdrop_path.as_deref(), "w1280"),
                    logo: pick_logo_url(details.images.as_ref()),
                    runtime: details.runtime.map(|minutes| format!("{} min", minutes)),
                    genres: details.genres.into_iter().map(|genre| genre.name).collect(),
                    popularity: details.popularity,
                    is_anime: false,
                    aliases: vec![details.title],
                    imdb_id: details
                        .external_ids
                        .as_ref()
                        .and_then(|external| external.imdb_id.clone()),
                    tmdb_id: Some(details.id),
                    anidb_id: None,
                    released: details.release_date.as_deref().and_then(iso_datetime),
                    imdb_rating: format_rating(details.vote_average),
                    director: directors_from_crew(
                        details.credits.as_ref().map(|credits| &credits.crew),
                    ),
                    cast: cast_names(details.credits.as_ref().map(|credits| &credits.cast)),
                    trailers: trailers_from_videos(details.videos.as_ref()),
                    episodes: Vec::new(),
                    updated_at: Utc::now(),
                };
                self.store_content(content).await?;
            }
            HydraKind::Series | HydraKind::Anime => {
                let url = format!(
                    "{}/tv/{}?api_key={}&append_to_response=external_ids",
                    TMDB_API_BASE, tmdb_id, api_key
                );
                let details: TmdbTvDetails = self.client.get(&url).send().await?.json().await?;
                let is_anime = kind == HydraKind::Anime
                    || (details.original_language.as_deref() == Some("ja")
                        && details
                            .genres
                            .iter()
                            .any(|genre| genre.name.eq_ignore_ascii_case("Animation")));
                let hydra_kind = if is_anime {
                    HydraKind::Anime
                } else {
                    HydraKind::Series
                };
                let hydra = HydraId::new(hydra_kind, HydraSource::Tmdb, tmdb_id.to_string());
                let mut season_episodes = Vec::new();
                for season in details
                    .seasons
                    .iter()
                    .filter(|season| season.season_number > 0)
                {
                    let season_url = format!(
                        "{}/tv/{}/season/{}?api_key={}",
                        TMDB_API_BASE, tmdb_id, season.season_number, api_key
                    );
                    let payload: TmdbSeasonDetails =
                        self.client.get(&season_url).send().await?.json().await?;
                    season_episodes.extend(payload.episodes.into_iter().map(|episode| {
                        IndexedEpisode {
                            season: season.season_number,
                            episode: episode.episode_number,
                            title: episode.name,
                            released_at: episode.air_date.as_deref().and_then(iso_datetime),
                            thumbnail: image_url(episode.still_path.as_deref(), "w500"),
                        }
                    }));
                }

                let episodes = if is_anime && !season_episodes.is_empty() {
                    season_episodes
                        .into_iter()
                        .enumerate()
                        .map(|(index, episode)| IndexedEpisode {
                            season: 1,
                            episode: (index + 1) as u32,
                            title: episode.title,
                            released_at: episode.released_at,
                            thumbnail: episode.thumbnail,
                        })
                        .collect()
                } else if !season_episodes.is_empty() {
                    season_episodes
                } else {
                    let mut fallback = Vec::new();
                    if is_anime && details.number_of_episodes > 0 {
                        for episode in 1..=details.number_of_episodes {
                            fallback.push(IndexedEpisode {
                                season: 1,
                                episode,
                                title: format!("Episode {}", episode),
                                released_at: None,
                                thumbnail: None,
                            });
                        }
                    }
                    fallback
                };

                let runtime = details
                    .episode_run_time
                    .as_ref()
                    .and_then(|times| times.first().copied())
                    .map(|minutes| format!("{} min", minutes));

                let mut aliases = vec![details.name.clone()];
                if let Some(pack) = featured_alias_pack(&details.name) {
                    aliases.extend(pack.iter().map(|alias| alias.to_string()));
                }
                aliases.sort();
                aliases.dedup();
                let genres = details
                    .genres
                    .iter()
                    .map(|genre| genre.name.clone())
                    .collect::<Vec<_>>();
                let director = series_directors(&details);
                let cast = cast_names(
                    details
                        .aggregate_credits
                        .as_ref()
                        .map(|credits| &credits.cast),
                );
                let trailers = trailers_from_videos(details.videos.as_ref());

                let content = IndexedContent {
                    hydra_id: hydra.to_string(),
                    kind: hydra_kind.as_str().to_string(),
                    content_type: "series".to_string(),
                    primary_source: "tmdb".to_string(),
                    primary_id: tmdb_id.to_string(),
                    title: details.name.clone(),
                    year: details
                        .first_air_date
                        .as_deref()
                        .and_then(|value| value.split('-').next())
                        .map(str::to_string),
                    description: details.overview.clone(),
                    poster: image_url(details.poster_path.as_deref(), "w500"),
                    background: image_url(details.backdrop_path.as_deref(), "w1280"),
                    logo: pick_logo_url(details.images.as_ref()),
                    runtime,
                    genres,
                    popularity: details.popularity,
                    is_anime,
                    aliases,
                    imdb_id: details
                        .external_ids
                        .as_ref()
                        .and_then(|external| external.imdb_id.clone()),
                    tmdb_id: Some(details.id),
                    anidb_id: None,
                    released: details.first_air_date.as_deref().and_then(iso_datetime),
                    imdb_rating: format_rating(details.vote_average),
                    director,
                    cast,
                    trailers,
                    episodes,
                    updated_at: Utc::now(),
                };
                self.store_content(content).await?;
            }
        }

        Ok(())
    }

    async fn search_and_store(&self, query: &str, content_type: &str) -> Result<()> {
        let Some(api_key) = self.tmdb_api_key.as_deref() else {
            return Ok(());
        };

        if query.starts_with("tt") {
            let endpoint = if content_type == "movie" {
                "movie_results"
            } else {
                "tv_results"
            };
            let url = format!(
                "{}/find/{}?api_key={}&external_source=imdb_id",
                TMDB_API_BASE, query, api_key
            );
            let payload: serde_json::Value = self.client.get(&url).send().await?.json().await?;
            let results = payload
                .get(endpoint)
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default();
            let genre_map = self.fetch_tmdb_genres(api_key).await?;
            for item in results {
                let summary: TmdbListItem = serde_json::from_value(item)?;
                let kind = if content_type == "movie" {
                    "movie"
                } else {
                    "series"
                };
                self.upsert_tmdb_summary(kind, &summary, &genre_map).await?;
            }
            return Ok(());
        }

        let endpoint = if content_type == "movie" {
            "search/movie"
        } else {
            "search/tv"
        };
        let url = format!(
            "{}/{}?api_key={}&query={}",
            TMDB_API_BASE,
            endpoint,
            api_key,
            urlencoding::encode(query)
        );
        let payload: TmdbListResponse = self.client.get(&url).send().await?.json().await?;
        let genre_map = self.fetch_tmdb_genres(api_key).await?;

        for item in payload.results.into_iter().take(10) {
            let kind = if content_type == "movie" {
                "movie"
            } else {
                "series"
            };
            self.upsert_tmdb_summary(kind, &item, &genre_map).await?;
        }

        Ok(())
    }

    async fn fetch_tmdb_genres(&self, api_key: &str) -> Result<HashMap<i64, String>> {
        let mut map = HashMap::new();
        for endpoint in ["genre/movie/list", "genre/tv/list"] {
            let url = format!("{}/{}?api_key={}", TMDB_API_BASE, endpoint, api_key);
            let payload: TmdbGenreResponse = self.client.get(&url).send().await?.json().await?;
            for genre in payload.genres {
                map.insert(genre.id, genre.name);
            }
        }
        Ok(map)
    }

    async fn store_content(&self, mut content: IndexedContent) -> Result<()> {
        content.aliases = dedupe_case_insensitive(content.aliases);

        if let Some(pool) = &self.pool {
            let genres_json = serde_json::to_value(&content.genres)?;
            let extra_json = serde_json::to_value(IndexedContentExtra {
                logo: content.logo.clone(),
                released: content.released.clone(),
                imdb_rating: content.imdb_rating.clone(),
                director: content.director.clone(),
                cast: content.cast.clone(),
                trailers: content.trailers.clone(),
            })?;
            sqlx::query(
                r#"
                INSERT INTO content_items (
                    hydra_id, kind, content_type, primary_source, primary_id, title, year,
                    description, poster, background, runtime, genres, extra, popularity, is_anime, updated_at
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7,
                    $8, $9, $10, $11, $12, $13, $14, $15, $16
                )
                ON CONFLICT (hydra_id) DO UPDATE SET
                    kind = EXCLUDED.kind,
                    content_type = EXCLUDED.content_type,
                    primary_source = EXCLUDED.primary_source,
                    primary_id = EXCLUDED.primary_id,
                    title = EXCLUDED.title,
                    year = EXCLUDED.year,
                    description = EXCLUDED.description,
                    poster = EXCLUDED.poster,
                    background = EXCLUDED.background,
                    runtime = EXCLUDED.runtime,
                    genres = EXCLUDED.genres,
                    extra = EXCLUDED.extra,
                    popularity = EXCLUDED.popularity,
                    is_anime = EXCLUDED.is_anime,
                    updated_at = EXCLUDED.updated_at
                "#,
            )
            .bind(&content.hydra_id)
            .bind(&content.kind)
            .bind(&content.content_type)
            .bind(&content.primary_source)
            .bind(&content.primary_id)
            .bind(&content.title)
            .bind(&content.year)
            .bind(&content.description)
            .bind(&content.poster)
            .bind(&content.background)
            .bind(&content.runtime)
            .bind(&genres_json)
            .bind(&extra_json)
            .bind(content.popularity)
            .bind(content.is_anime)
            .bind(content.updated_at)
            .execute(pool)
            .await?;

            sqlx::query("DELETE FROM content_aliases WHERE hydra_id = $1")
                .bind(&content.hydra_id)
                .execute(pool)
                .await?;
            for alias in &content.aliases {
                sqlx::query(
                    "INSERT INTO content_aliases (hydra_id, alias, source) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                )
                .bind(&content.hydra_id)
                .bind(alias)
                .bind(if content.is_anime { "anime" } else { "tmdb" })
                .execute(pool)
                .await?;
            }

            sqlx::query(
                r#"
                INSERT INTO content_external_ids (hydra_id, imdb_id, tmdb_id, anidb_id)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (hydra_id) DO UPDATE SET
                    imdb_id = EXCLUDED.imdb_id,
                    tmdb_id = EXCLUDED.tmdb_id,
                    anidb_id = EXCLUDED.anidb_id
                "#,
            )
            .bind(&content.hydra_id)
            .bind(&content.imdb_id)
            .bind(content.tmdb_id)
            .bind(content.anidb_id)
            .execute(pool)
            .await?;

            sqlx::query("DELETE FROM content_episodes WHERE hydra_id = $1")
                .bind(&content.hydra_id)
                .execute(pool)
                .await?;
            for episode in &content.episodes {
                sqlx::query(
                    r#"
                    INSERT INTO content_episodes (hydra_id, season, episode, title, released_at, thumbnail)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    ON CONFLICT (hydra_id, season, episode) DO UPDATE SET
                        title = EXCLUDED.title,
                        released_at = EXCLUDED.released_at,
                        thumbnail = EXCLUDED.thumbnail
                    "#,
                )
                .bind(&content.hydra_id)
                .bind(episode.season as i32)
                .bind(episode.episode as i32)
                .bind(&episode.title)
                .bind(&episode.released_at)
                .bind(&episode.thumbnail)
                .execute(pool)
                .await?;
            }
        }

        let mut items = self.items.write().await;
        items.insert(content.hydra_id.clone(), content);
        Ok(())
    }

    async fn load_from_store(&self) -> Result<()> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };

        let item_rows = sqlx::query_as::<_, (
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            serde_json::Value,
            serde_json::Value,
            f64,
            bool,
            DateTime<Utc>,
        )>(
            r#"
            SELECT hydra_id, kind, content_type, primary_source, primary_id, title, year,
                   description, poster, background, runtime, genres, extra, popularity, is_anime, updated_at
            FROM content_items
            "#,
        )
        .fetch_all(pool)
        .await?;

        let alias_rows =
            sqlx::query_as::<_, (String, String)>("SELECT hydra_id, alias FROM content_aliases")
                .fetch_all(pool)
                .await?;
        let episode_rows = sqlx::query_as::<
            _,
            (String, i32, i32, String, Option<String>, Option<String>),
        >(
            "SELECT hydra_id, season, episode, title, released_at, thumbnail FROM content_episodes",
        )
        .fetch_all(pool)
        .await?;
        let external_rows =
            sqlx::query_as::<_, (String, Option<String>, Option<i64>, Option<i32>)>(
                "SELECT hydra_id, imdb_id, tmdb_id, anidb_id FROM content_external_ids",
            )
            .fetch_all(pool)
            .await?;

        let mut alias_map: HashMap<String, Vec<String>> = HashMap::new();
        for (hydra_id, alias) in alias_rows {
            alias_map.entry(hydra_id).or_default().push(alias);
        }

        let mut episode_map: HashMap<String, Vec<IndexedEpisode>> = HashMap::new();
        for (hydra_id, season, episode, title, released_at, thumbnail) in episode_rows {
            episode_map
                .entry(hydra_id)
                .or_default()
                .push(IndexedEpisode {
                    season: season as u32,
                    episode: episode as u32,
                    title,
                    released_at,
                    thumbnail,
                });
        }

        let mut external_map: HashMap<String, (Option<String>, Option<i64>, Option<i32>)> =
            HashMap::new();
        for (hydra_id, imdb_id, tmdb_id, anidb_id) in external_rows {
            external_map.insert(hydra_id, (imdb_id, tmdb_id, anidb_id));
        }

        let mut items = self.items.write().await;
        for row in item_rows {
            let genres: Vec<String> = serde_json::from_value(row.11).unwrap_or_default();
            let extra: IndexedContentExtra = serde_json::from_value(row.12).unwrap_or_default();
            let (imdb_id, tmdb_id, anidb_id) =
                external_map.remove(&row.0).unwrap_or((None, None, None));
            items.insert(
                row.0.clone(),
                IndexedContent {
                    hydra_id: row.0.clone(),
                    kind: row.1,
                    content_type: row.2,
                    primary_source: row.3,
                    primary_id: row.4,
                    title: row.5,
                    year: row.6,
                    description: row.7,
                    poster: row.8,
                    background: row.9,
                    runtime: row.10,
                    logo: extra.logo,
                    genres,
                    popularity: row.13,
                    is_anime: row.14,
                    aliases: alias_map.remove(&row.0).unwrap_or_default(),
                    imdb_id,
                    tmdb_id,
                    anidb_id,
                    released: extra.released,
                    imdb_rating: extra.imdb_rating,
                    director: extra.director,
                    cast: extra.cast,
                    trailers: extra.trailers,
                    episodes: episode_map.remove(&row.0).unwrap_or_default(),
                    updated_at: row.15,
                },
            );
        }

        Ok(())
    }

    async fn import_anidb_titles(&self) -> Result<()> {
        let Some(source) = self.anidb_titles_source.as_deref() else {
            return Ok(());
        };

        let bytes = if source.starts_with("http://") || source.starts_with("https://") {
            self.client
                .get(source)
                .send()
                .await?
                .bytes()
                .await?
                .to_vec()
        } else {
            std::fs::read(source)?
        };

        let xml = gunzip_or_plain(&bytes)?;
        let mut reader = quick_xml::reader::Reader::from_str(&xml);
        reader.trim_text(true);

        let mut current_aid: Option<String> = None;
        let mut current_aliases: Vec<String> = Vec::new();
        let mut in_title = false;
        let mut current_tag = String::new();
        let mut anime_aliases: Vec<(String, Vec<String>)> = Vec::new();

        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Start(event)) => {
                    let name = String::from_utf8_lossy(event.local_name().as_ref()).to_string();
                    current_tag = name.clone();
                    if name == "anime" {
                        current_aliases.clear();
                        current_aid = event
                            .attributes()
                            .flatten()
                            .find(|attr| attr.key.as_ref() == b"aid")
                            .map(|attr| String::from_utf8_lossy(attr.value.as_ref()).to_string());
                    } else if name == "title" {
                        in_title = true;
                    }
                }
                Ok(quick_xml::events::Event::End(event)) => {
                    let name = String::from_utf8_lossy(event.local_name().as_ref()).to_string();
                    if name == "anime" {
                        if let Some(aid) = current_aid.take() {
                            anime_aliases
                                .push((aid, dedupe_case_insensitive(current_aliases.clone())));
                        }
                    } else if name == "title" {
                        in_title = false;
                    }
                    current_tag.clear();
                }
                Ok(quick_xml::events::Event::Text(event)) => {
                    if in_title && current_tag == "title" {
                        let value = String::from_utf8_lossy(event.as_ref()).trim().to_string();
                        if !value.is_empty() {
                            current_aliases.push(value);
                        }
                    }
                }
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!("AniDB alias import failed: {}", error);
                    break;
                }
            }
        }

        let mut items = self.items.write().await;
        for (_aid, aliases) in anime_aliases {
            if let Some(item) = items.values_mut().find(|item| {
                item.is_anime
                    && aliases.iter().any(|alias| {
                        normalize(alias) == normalize(&item.title)
                            || item
                                .aliases
                                .iter()
                                .any(|known| normalize(alias) == normalize(known))
                    })
            }) {
                item.aliases.extend(aliases.clone());
                item.aliases = dedupe_case_insensitive(item.aliases.clone());
            }
        }

        Ok(())
    }

    async fn import_imdb_aliases(&self) -> Result<()> {
        let Some(akas_source) = self.imdb_akas_url.as_deref() else {
            return Ok(());
        };

        let bytes = if akas_source.starts_with("http://") || akas_source.starts_with("https://") {
            self.client
                .get(akas_source)
                .send()
                .await?
                .bytes()
                .await?
                .to_vec()
        } else {
            std::fs::read(akas_source)?
        };
        let tsv = gunzip_or_plain(&bytes)?;
        let mut csv = csv::ReaderBuilder::new()
            .delimiter(b'\t')
            .has_headers(true)
            .from_reader(tsv.as_bytes());

        let basics_filter = self.imdb_basics_url.as_deref();
        if basics_filter.is_some() {
            tracing::info!("IMDb basics source configured for future enrichment");
        }

        let mut alias_map: HashMap<String, Vec<String>> = HashMap::new();
        for record in csv.records().flatten().take(50_000) {
            let Some(title_id) = record.get(0) else {
                continue;
            };
            let Some(alias) = record.get(2) else {
                continue;
            };
            if !title_id.starts_with("tt") || alias == "\\N" {
                continue;
            }
            alias_map
                .entry(title_id.to_string())
                .or_default()
                .push(alias.to_string());
        }

        let mut items = self.items.write().await;
        for item in items.values_mut() {
            if let Some(imdb_id) = item.imdb_id.as_ref() {
                if let Some(aliases) = alias_map.get(imdb_id) {
                    item.aliases.extend(aliases.clone());
                    item.aliases = dedupe_case_insensitive(item.aliases.clone());
                }
            }
        }

        Ok(())
    }

    async fn ensure_tables(pool: &PgPool) -> Result<()> {
        for statement in [
            r#"
            CREATE TABLE IF NOT EXISTS content_items (
                hydra_id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                content_type TEXT NOT NULL,
                primary_source TEXT NOT NULL,
                primary_id TEXT NOT NULL,
                title TEXT NOT NULL,
                year TEXT,
                description TEXT,
                poster TEXT,
                background TEXT,
                runtime TEXT,
                genres JSONB NOT NULL DEFAULT '[]'::jsonb,
                extra JSONB NOT NULL DEFAULT '{}'::jsonb,
                popularity DOUBLE PRECISION NOT NULL DEFAULT 0,
                is_anime BOOLEAN NOT NULL DEFAULT FALSE,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS content_aliases (
                hydra_id TEXT NOT NULL,
                alias TEXT NOT NULL,
                source TEXT NOT NULL,
                PRIMARY KEY (hydra_id, alias)
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS content_external_ids (
                hydra_id TEXT PRIMARY KEY,
                imdb_id TEXT,
                tmdb_id BIGINT,
                anidb_id INTEGER
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS content_episodes (
                hydra_id TEXT NOT NULL,
                season INTEGER NOT NULL,
                episode INTEGER NOT NULL,
                title TEXT NOT NULL,
                released_at TEXT,
                thumbnail TEXT,
                PRIMARY KEY (hydra_id, season, episode)
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS sync_state (
                key TEXT PRIMARY KEY,
                value JSONB NOT NULL DEFAULT '{}'::jsonb,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
            r#"
            ALTER TABLE content_items
            ADD COLUMN IF NOT EXISTS extra JSONB NOT NULL DEFAULT '{}'::jsonb
            "#,
        ] {
            sqlx::query(statement).execute(pool).await?;
        }

        Ok(())
    }

    fn to_meta_item(&self, item: &IndexedContent) -> MetaItem {
        let videos = item
            .episodes
            .iter()
            .map(|episode| MetaVideo {
                id: HydraId::new(
                    if item.is_anime {
                        HydraKind::Anime
                    } else if item.content_type == "movie" {
                        HydraKind::Movie
                    } else {
                        HydraKind::Series
                    },
                    match item.primary_source.as_str() {
                        "tmdb" => HydraSource::Tmdb,
                        "tt" => HydraSource::Tt,
                        _ => HydraSource::Anidb,
                    },
                    item.primary_id.clone(),
                )
                .with_episode(episode.season, episode.episode)
                .to_string(),
                title: episode.title.clone(),
                season: episode.season,
                episode: episode.episode,
                thumbnail: episode.thumbnail.clone(),
                released: episode.released_at.clone(),
            })
            .collect();

        MetaItem {
            id: item.hydra_id.clone(),
            content_type: item.content_type.clone(),
            name: item.title.clone(),
            poster: item.poster.clone(),
            logo: item.logo.clone(),
            background: item.background.clone(),
            description: item.description.clone(),
            genres: item.genres.clone(),
            release_info: item.year.clone(),
            runtime: item.runtime.clone(),
            released: item.released.clone(),
            imdb_rating: item.imdb_rating.clone(),
            director: item.director.clone(),
            cast: item.cast.clone(),
            trailers: item.trailers.clone(),
            videos,
        }
    }

    fn build_stream_queries(
        &self,
        item: &IndexedContent,
        season: Option<u32>,
        episode: Option<u32>,
    ) -> Vec<String> {
        let mut queries = item.aliases.clone();
        if !queries
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&item.title))
        {
            queries.insert(0, item.title.clone());
        }

        let mut out = Vec::new();
        for title in dedupe_case_insensitive(queries) {
            out.push(title.clone());
            if let Some(year) = item.year.as_ref() {
                out.push(format!("{} {}", title, year));
            }

            if let Some(episode) = episode {
                let season = season.unwrap_or(1);
                if item.is_anime {
                    out.push(format!("{} {:02}", title, episode));
                    out.push(format!("{} - {:02}", title, episode));
                    out.push(format!("{} E{:02}", title, episode));
                    out.push(format!("{} EP{:02}", title, episode));
                } else {
                    out.push(format!("{} S{:02}E{:02}", title, season, episode));
                    out.push(format!("{} {}x{:02}", title, season, episode));
                }
            }
        }

        dedupe_case_insensitive(out)
    }

    fn catalog_matches(item: &IndexedContent, catalog_id: &str) -> bool {
        match catalog_id {
            "hydrogen_movie_trending" | "hydrogen_movie_popular" | "hydrogen_movie_new" => {
                item.content_type == "movie"
            }
            "hydrogen_series_trending" | "hydrogen_series_popular" | "hydrogen_series_recent" => {
                item.content_type == "series" && !item.is_anime
            }
            "hydrogen_anime_trending" | "hydrogen_anime_popular" | "hydrogen_anime_recent" => {
                item.is_anime
            }
            "hydrogen_movie_search" => item.content_type == "movie",
            "hydrogen_series_search" => item.content_type == "series",
            _ => true,
        }
    }

    fn search_score(item: &IndexedContent, query: &str) -> i32 {
        let mut best = if normalize(&item.title).contains(query) {
            100
        } else {
            0
        };
        for alias in &item.aliases {
            let normalized = normalize(alias);
            let mut score = 0;
            if normalized == query {
                score += 120;
            } else if normalized.starts_with(query) {
                score += 100;
            } else if normalized.contains(query) {
                score += 80;
            }
            if score > best {
                best = score;
            }
        }
        best
    }
}

fn image_url(path: Option<&str>, size: &str) -> Option<String> {
    path.map(|path| format!("https://image.tmdb.org/t/p/{size}{path}"))
}

fn pick_logo_url(images: Option<&TmdbImages>) -> Option<String> {
    let mut logos = images?.logos.iter().collect::<Vec<_>>();
    logos.sort_by(|a, b| {
        logo_rank(b)
            .cmp(&logo_rank(a))
            .then_with(|| b.vote_average.total_cmp(&a.vote_average))
    });
    logos
        .first()
        .and_then(|logo| image_url(Some(logo.file_path.as_str()), "original"))
}

fn logo_rank(logo: &TmdbImageAsset) -> i32 {
    let language_rank = match logo.iso_639_1.as_deref() {
        Some("en") => 3,
        None => 2,
        _ => 1,
    };
    let file_type_rank = match logo.file_type.as_deref() {
        Some(".png") => 2,
        Some(".svg") => 1,
        _ => 0,
    };
    language_rank * 10 + file_type_rank
}

fn format_rating(vote_average: f64) -> Option<String> {
    (vote_average > 0.0).then(|| format!("{vote_average:.1}"))
}

fn iso_datetime(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed != "\\N").then(|| format!("{trimmed}T00:00:00.000Z"))
}

fn directors_from_crew(crew: Option<&Vec<TmdbCrewMember>>) -> Vec<String> {
    dedupe_case_insensitive(
        crew.into_iter()
            .flat_map(|crew| crew.iter())
            .filter(|member| member.job.as_deref() == Some("Director"))
            .map(|member| member.name.clone())
            .collect(),
    )
}

fn cast_names(cast: Option<&Vec<TmdbCastMember>>) -> Vec<String> {
    cast.into_iter()
        .flat_map(|cast| cast.iter().take(8))
        .map(|member| member.name.clone())
        .collect()
}

fn series_directors(details: &TmdbTvDetails) -> Vec<String> {
    let mut names = details
        .created_by
        .iter()
        .map(|person| person.name.clone())
        .collect::<Vec<_>>();
    if let Some(credits) = details.aggregate_credits.as_ref() {
        names.extend(
            credits
                .crew
                .iter()
                .filter(|member| member.job.as_deref() == Some("Director"))
                .map(|member| member.name.clone()),
        );
    }
    dedupe_case_insensitive(names)
}

fn trailers_from_videos(videos: Option<&TmdbVideos>) -> Vec<MetaTrailer> {
    videos
        .into_iter()
        .flat_map(|videos| videos.results.iter())
        .filter(|video| video.site.eq_ignore_ascii_case("YouTube"))
        .filter(|video| matches!(video.video_type.as_str(), "Trailer" | "Teaser"))
        .take(3)
        .map(|video| MetaTrailer {
            source: format!("https://www.youtube.com/watch?v={}", video.key),
            trailer_type: if video.official {
                "Trailer".to_string()
            } else {
                video.video_type.clone()
            },
        })
        .collect()
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn dedupe_case_insensitive(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.to_lowercase()))
        .collect()
}

fn featured_alias_pack(title: &str) -> Option<&'static [&'static str]> {
    match title {
        "Attack on Titan" => Some(&["Shingeki no Kyojin", "進撃の巨人", "SnK"]),
        "Death Note" => Some(&["デスノート"]),
        "One Piece" => Some(&["ワンピース"]),
        _ => None,
    }
}

fn gunzip_or_plain(bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::read::GzDecoder::new(bytes);
        let mut output = String::new();
        decoder.read_to_string(&mut output)?;
        Ok(output)
    } else {
        Ok(String::from_utf8_lossy(bytes).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_prefers_exact_alias() {
        let item = IndexedContent {
            hydra_id: "hydra:anime:tmdb:1".to_string(),
            kind: "anime".to_string(),
            content_type: "series".to_string(),
            primary_source: "tmdb".to_string(),
            primary_id: "1".to_string(),
            title: "Attack on Titan".to_string(),
            year: Some("2013".to_string()),
            description: None,
            poster: None,
            background: None,
            logo: None,
            runtime: None,
            genres: vec!["Animation".to_string()],
            popularity: 100.0,
            is_anime: true,
            aliases: vec!["Shingeki no Kyojin".to_string()],
            imdb_id: None,
            tmdb_id: Some(1),
            anidb_id: None,
            released: None,
            imdb_rating: None,
            director: Vec::new(),
            cast: Vec::new(),
            trailers: Vec::new(),
            episodes: vec![],
            updated_at: Utc::now(),
        };

        assert!(MetadataIndex::search_score(&item, "shingeki no kyojin") > 100);
    }
}
