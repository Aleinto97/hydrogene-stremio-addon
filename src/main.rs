use axum::{
    extract::{Path, Query, State},
    response::{Json, Redirect},
    routing::get,
    Router,
};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

use hydrogene::debrid;
use hydrogene::hydra::HydraId;
use hydrogene::matching;
use hydrogene::meta_index::{CatalogMeta, MetaItem};
use hydrogene::metadata;
use hydrogene::scrapers;
use hydrogene::stremio_format::{StremioStream, TorrentInfo};
use hydrogene::ResolveResult;
use hydrogene::{MetadataCache, MetadataIndex};

use metadata::MetadataClient;
use scrapers::{ScrapedTorrent, ScraperManager};

#[derive(Clone)]
struct AppState {
    scraper_manager: Arc<ScraperManager>,
    debrid_client: Arc<debrid::RealDebridClient>,
    metadata_client: Arc<MetadataClient>,
    metadata_index: Arc<MetadataIndex>,
}

static YEAR_TOKEN_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:19|20)\d{2}\b").expect("invalid year regex"));
static SXXEXX_QUERY_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bS\d{1,2}E\d{1,3}\b").expect("invalid sxxexx regex"));
static SEASON_X_EP_QUERY_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b\d{1,2}x\d{1,3}\b").expect("invalid 1x01 regex"));
static SEASON_ONLY_QUERY_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bSEASON\s+\d{1,2}\b").expect("invalid season regex"));
static EPISODE_WORD_QUERY_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:EPISODE|EP|E)\s*\d{1,3}\b").expect("invalid episode regex"));
static EXPLICIT_EPISODE_HINT_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\bS\d{1,2}E\d{1,3}\b|\b\d{1,2}x\d{1,3}\b|\b(?:EPISODE|EP|E)\s*\d{1,3}\b|\[\d{1,3}(?:v\d)?\]|\s-\s*\d{1,3}\b|\b\d{1,3}\b$",
    )
    .expect("invalid explicit episode regex")
});
static EXPLICIT_EPISODE_TITLE_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\bS\d{1,2}E\d{1,3}\b|\b\d{1,2}x\d{1,3}\b|\b(?:EPISODE|EP|E)\s*\d{1,3}\b|\[\d{1,3}(?:v\d)?\]|\s-\s*\d{1,3}(?:v\d)?(?:\s|$)",
    )
    .expect("invalid explicit episode title regex")
});
static TRAILING_EPISODE_QUERY_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:\s*-\s*\d{1,3}\s*$)|(?:\b\d{2,3}\b$)")
        .expect("invalid trailing episode regex")
});
static MULTISPACE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s+").expect("invalid whitespace regex"));

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        .add_directive("tower_http=info".parse().unwrap())
        .add_directive("axum=info".parse().unwrap())
        .add_directive("scraper=info".parse().unwrap())
        .add_directive("selectors=info".parse().unwrap())
        .add_directive("html5ever=info".parse().unwrap())
        .add_directive("tendril=info".parse().unwrap())
        .add_directive("hyper=info".parse().unwrap())
        .add_directive("h2=info".parse().unwrap())
        .add_directive("rustls=info".parse().unwrap())
        .add_directive("sqlx=warn".parse().unwrap());

    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("Starting Stremio Addon Server...");

    let http_client = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .build()?,
    );
    info!("HTTP client initialized with connection pooling");

    let scraper_manager = Arc::new(ScraperManager::new()?);

    let debrid_client = Arc::new(debrid::RealDebridClient::new()?);

    let metadata_client = if let Ok(database_url) = std::env::var("DATABASE_URL") {
        info!("Initializing metadata cache with PostgreSQL...");
        match MetadataCache::new(&database_url).await {
            Ok(cache) => {
                info!("Metadata cache initialized successfully");
                MetadataClient::new(http_client.clone())?.with_cache(Arc::new(cache))
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to initialize metadata cache: {}. Continuing without cache.",
                    e
                );
                MetadataClient::new(http_client.clone())?
            }
        }
    } else {
        info!("No DATABASE_URL found, running without metadata cache");
        MetadataClient::new(http_client.clone())?
    };
    let metadata_index = Arc::new(MetadataIndex::new(http_client.clone()).await?);
    {
        let metadata_index = metadata_index.clone();
        tokio::spawn(async move {
            if let Err(error) = metadata_index.bootstrap().await {
                tracing::warn!("Metadata bootstrap failed: {}", error);
            }
        });
    }
    metadata_index.clone().spawn_refresh_task();

    let app_state = AppState {
        scraper_manager,
        debrid_client,
        metadata_client: Arc::new(metadata_client),
        metadata_index,
    };

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/manifest.json", get(manifest_handler))
        .route("/manifest-meta.json", get(meta_manifest_handler))
        .route("/catalog/:type/:id.json", get(catalog_handler))
        .route("/catalog/:type/:id/:extra.json", get(catalog_handler_extra))
        .route("/meta/:type/:id.json", get(meta_handler))
        .route("/stream/:type/:id.json", get(stream_handler))
        .route("/resolve/:hash/:id", get(resolve_handler))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                let uri = request.uri().to_string();
                if uri == "/" {
                    tracing::Span::none()
                } else {
                    tracing::info_span!(
                        "request",
                        method = %request.method(),
                        uri = %uri,
                    )
                }
            }),
        )
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(app_state);

    let app = app.layer(axum::middleware::from_fn(timeout_middleware));

    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()?;

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn timeout_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let uri = req.uri().to_string();

    if uri == "/"
        || uri == "/manifest.json"
        || uri == "/manifest-meta.json"
        || uri.starts_with("/catalog/")
        || uri.starts_with("/meta/")
        || uri.starts_with("/resolve/")
        || uri.starts_with("/cached/")
    {
        return next.run(req).await;
    }

    let timeout_secs = configured_request_timeout_seconds(&uri);

    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), next.run(req)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!("Request timeout for {} after {}s", uri, timeout_secs);
            let body = axum::body::Body::from(r#"{"streams": []}"#);
            axum::response::Response::builder()
                .status(200)
                .header("Content-Type", "application/json")
                .body(body)
                .unwrap()
        }
    }
}

fn configured_request_timeout_seconds(uri: &str) -> u64 {
    let configured = env_u64("REQUEST_TIMEOUT_SECONDS", 8);

    if uri.starts_with("/stream/series/") {
        configured.max(10)
    } else {
        configured
    }
}

async fn root_handler() -> &'static str {
    "Hydrogen Stremio Addon - Use /manifest.json for Stremio"
}

#[derive(serde::Serialize)]
struct Manifest {
    id: String,
    version: String,
    name: String,
    description: String,
    types: Vec<String>,
    catalogs: Vec<serde_json::Value>,
    resources: Vec<String>,
    id_prefixes: Vec<String>,
    behavior_hints: serde_json::Value,
}

async fn manifest_handler() -> Json<Manifest> {
    let addon_name =
        std::env::var("ADDON_NAME").unwrap_or_else(|_| "Hydrogen Torrents".to_string());
    let addon_desc = std::env::var("ADDON_DESCRIPTION")
        .unwrap_or_else(|_| "High-performance torrent scraper".to_string());
    let addon_id = std::env::var("ADDON_ID").unwrap_or_else(|_| "ai.hydrogen.stream".to_string());
    let addon_version = std::env::var("ADDON_VERSION").unwrap_or_else(|_| "0.1.0".to_string());

    Json(Manifest {
        id: addon_id,
        version: addon_version,
        name: addon_name,
        description: addon_desc,
        types: vec!["movie".to_string(), "series".to_string()],
        catalogs: vec![],
        resources: vec!["stream".to_string()],
        id_prefixes: vec!["tt".to_string(), "anilist".to_string(), "hydra".to_string()],
        behavior_hints: serde_json::json!({
            "configurable": false,
            "configurationRequired": false
        }),
    })
}

#[derive(serde::Serialize)]
struct MetaManifest {
    id: String,
    version: String,
    name: String,
    description: String,
    types: Vec<String>,
    catalogs: Vec<serde_json::Value>,
    resources: Vec<serde_json::Value>,
    id_prefixes: Vec<String>,
    behavior_hints: serde_json::Value,
}

async fn meta_manifest_handler() -> Json<MetaManifest> {
    let addon_version = std::env::var("ADDON_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    Json(MetaManifest {
        id: "ai.hydrogen.meta".to_string(),
        version: addon_version,
        name: "Hydrogen Meta".to_string(),
        description: "Meta and catalog addon that neutralizes Cinemeta entrypoints".to_string(),
        types: vec!["movie".to_string(), "series".to_string()],
        catalogs: vec![
            catalog_manifest(
                "movie",
                "hydrogen_movie_trending",
                "Hydrogen Movies Trending",
                false,
            ),
            catalog_manifest(
                "movie",
                "hydrogen_movie_popular",
                "Hydrogen Movies Popular",
                false,
            ),
            catalog_manifest("movie", "hydrogen_movie_new", "Hydrogen Movies New", false),
            catalog_manifest(
                "movie",
                "hydrogen_movie_search",
                "Hydrogen Movies Search",
                true,
            ),
            catalog_manifest(
                "series",
                "hydrogen_series_trending",
                "Hydrogen Series Trending",
                false,
            ),
            catalog_manifest(
                "series",
                "hydrogen_series_popular",
                "Hydrogen Series Popular",
                false,
            ),
            catalog_manifest(
                "series",
                "hydrogen_series_recent",
                "Hydrogen Series Recent",
                false,
            ),
            catalog_manifest(
                "series",
                "hydrogen_series_search",
                "Hydrogen Series Search",
                true,
            ),
            catalog_manifest(
                "series",
                "hydrogen_anime_trending",
                "Hydrogen Anime Trending",
                false,
            ),
            catalog_manifest(
                "series",
                "hydrogen_anime_popular",
                "Hydrogen Anime Popular",
                false,
            ),
            catalog_manifest(
                "series",
                "hydrogen_anime_recent",
                "Hydrogen Anime Recent",
                false,
            ),
        ],
        resources: vec![
            serde_json::json!({"name": "catalog", "types": ["movie", "series"], "idPrefixes": ["hydra", "tt", "tmdb", "anidb"]}),
            serde_json::json!({"name": "meta", "types": ["movie", "series"], "idPrefixes": ["hydra", "tt", "tmdb", "anidb"]}),
        ],
        id_prefixes: vec![
            "hydra".to_string(),
            "tt".to_string(),
            "tmdb".to_string(),
            "anidb".to_string(),
        ],
        behavior_hints: serde_json::json!({
            "configurable": false,
            "configurationRequired": false
        }),
    })
}

fn catalog_manifest(
    content_type: &str,
    id: &str,
    name: &str,
    searchable: bool,
) -> serde_json::Value {
    let mut extra = vec![serde_json::json!({"name": "skip", "isRequired": false})];
    if searchable {
        extra.insert(
            0,
            serde_json::json!({"name": "search", "isRequired": false}),
        );
    } else {
        extra.push(serde_json::json!({"name": "genre", "isRequired": false}));
    }

    serde_json::json!({
        "type": content_type,
        "id": id,
        "name": name,
        "extra": extra
    })
}

#[derive(serde::Serialize)]
struct CatalogResponse {
    metas: Vec<CatalogMeta>,
}

#[derive(serde::Serialize)]
struct MetaResponse {
    meta: MetaItem,
}

async fn catalog_handler(
    State(state): State<AppState>,
    Path((content_type, id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<CatalogResponse> {
    catalog_response(state, content_type, id, params).await
}

async fn catalog_handler_extra(
    State(state): State<AppState>,
    Path((content_type, id, extra)): Path<(String, String, String)>,
    Query(mut params): Query<HashMap<String, String>>,
) -> Json<CatalogResponse> {
    merge_extra_segment(&mut params, &extra);
    catalog_response(state, content_type, id, params).await
}

fn merge_extra_segment(params: &mut HashMap<String, String>, extra: &str) {
    for pair in extra.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let decoded_key = urlencoding::decode(key)
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| key.to_string());
        let decoded_value = urlencoding::decode(value)
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| value.to_string());
        params.entry(decoded_key).or_insert(decoded_value);
    }
}

async fn catalog_response(
    state: AppState,
    content_type: String,
    id: String,
    params: HashMap<String, String>,
) -> Json<CatalogResponse> {
    let search = params.get("search").map(String::as_str);
    let genre = params.get("genre").map(String::as_str);
    let skip = params
        .get("skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let metas = state
        .metadata_index
        .catalog(&content_type, &id, search, genre, skip, 50)
        .await
        .unwrap_or_default();

    Json(CatalogResponse { metas })
}

async fn meta_handler(
    State(state): State<AppState>,
    Path((content_type, id)): Path<(String, String)>,
) -> Result<Json<MetaResponse>, axum::http::StatusCode> {
    let id = id.trim_end_matches(".json").to_string();
    if content_type != "movie" && content_type != "series" {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    match state.metadata_index.get_meta(&id, &content_type).await {
        Ok(Some(meta)) => Ok(Json(MetaResponse { meta })),
        Ok(None) => Err(axum::http::StatusCode::NOT_FOUND),
        Err(_) => Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn request_targets(id: &str) -> (String, Option<u32>, Option<u32>) {
    if let Some(hydra) = HydraId::parse(id) {
        return (hydra.base_key(), hydra.season, hydra.episode);
    }

    if id.starts_with("anilist:") {
        let parts: Vec<&str> = id.split(':').collect();
        let episode = parts.get(2).and_then(|value| value.parse::<u32>().ok());
        return (id.to_string(), Some(1), episode);
    }

    if id.contains(':') {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() >= 3 {
            return (
                parts[0].to_string(),
                parts[1].parse::<u32>().ok(),
                parts[2].parse::<u32>().ok(),
            );
        }
    }

    (id.to_string(), None, None)
}

#[derive(serde::Serialize)]
struct StreamResponse {
    streams: Vec<StremioStream>,
}

async fn stream_handler(
    State(state): State<AppState>,
    Path((content_type, id)): Path<(String, String)>,
) -> Json<StreamResponse> {
    let id = id.trim_end_matches(".json").to_string();
    let (metadata_id, target_season, target_episode) = request_targets(&id);

    info!("Stream request: type={}, id={}", content_type, id);

    let (_metadata_title, torrents, match_titles, target_year, target_season, target_episode) = {
        let metadata_future = async {
            if HydraId::parse(&id).is_some() {
                match state.metadata_index.resolve_stream_metadata(&id).await {
                    Ok(Some(meta)) => (meta.title, meta.search_queries, meta.year),
                    Ok(None) | Err(_) => (metadata_id.clone(), vec![metadata_id.clone()], None),
                }
            } else if metadata_id.starts_with("anilist:") {
                match state
                    .metadata_client
                    .lookup_by_imdb(&metadata_id, &content_type)
                    .await
                {
                    Ok(meta) => (meta.title, meta.search_queries, meta.year),
                    Err(_) => (metadata_id.clone(), vec![metadata_id.clone()], None),
                }
            } else if metadata_id.starts_with("tt") {
                let full_metadata_id = if id.contains(':') && !id.starts_with("anilist:") {
                    id.clone()
                } else {
                    metadata_id.clone()
                };

                match state
                    .metadata_client
                    .lookup_by_imdb(&full_metadata_id, &content_type)
                    .await
                {
                    Ok(meta) => (meta.title, meta.search_queries, meta.year),
                    Err(e) => {
                        tracing::error!(
                            "Metadata lookup failed for {}: {}. Falling back to ID search.",
                            full_metadata_id,
                            e
                        );
                        (metadata_id.clone(), vec![metadata_id.clone()], None)
                    }
                }
            } else {
                (metadata_id.clone(), vec![metadata_id.clone()], None)
            }
        };

        let (metadata_title, search_queries, target_year) = metadata_future.await;
        let target_year_u32 = target_year.as_ref().and_then(|y| y.parse::<u32>().ok());
        let match_titles = build_match_titles(
            &metadata_title,
            &search_queries,
            target_season,
            target_episode,
        );
        let search_queries = select_search_queries(
            search_queries,
            &metadata_title,
            target_year_u32,
            target_season,
            target_episode,
        );

        let mut unique = scrape_queries_progressively(
            state.scraper_manager.clone(),
            &content_type,
            &search_queries,
            &match_titles,
            target_year_u32,
            target_season,
            target_episode,
        )
        .await;

        unique.sort_by(|a, b| {
            let b_score = best_match_score(
                &match_titles,
                target_year_u32,
                target_season,
                target_episode,
                b,
            );
            let a_score = best_match_score(
                &match_titles,
                target_year_u32,
                target_season,
                target_episode,
                a,
            );
            b_score.cmp(&a_score)
        });
        unique.truncate(80);

        info!("Found {} unique torrents for {}", unique.len(), id);
        (
            metadata_title,
            unique,
            match_titles,
            target_year,
            target_season,
            target_episode,
        )
    };

    let base_url = std::env::var("BASE_URL")
        .unwrap_or_else(|_| "http://torrentio-stack-aleinto97-54335f00.koyeb.app".to_string());

    let min_seeders = configured_min_seeders();

    let total_scraped = torrents.len();

    // Parse target year from string to u32 for matching
    let target_year_u32 = target_year.as_ref().and_then(|y| y.parse::<u32>().ok());

    // Score all torrents (don't filter out season packs)
    let mut scored_torrents: Vec<(ScrapedTorrent, i32)> = torrents
        .into_iter()
        .filter(|t| {
            torrent_passes_match_filters(
                t,
                &match_titles,
                target_year_u32,
                target_season,
                target_episode,
                min_seeders,
            )
        })
        .map(|t| {
            // Use advanced matching with fuzzy logic
            let score = best_match_score(
                &match_titles,
                target_year_u32,
                target_season,
                target_episode,
                &t,
            );
            (t, score)
        })
        .collect();

    scored_torrents.sort_by(|a, b| {
        let a_quality = quality_bucket(&a.0.title);
        let b_quality = quality_bucket(&b.0.title);

        b.1.cmp(&a.1)
            .then_with(|| b_quality.cmp(&a_quality))
            .then_with(|| b.0.size_bytes.cmp(&a.0.size_bytes))
            .then_with(|| b.0.seeders.cmp(&a.0.seeders))
    });

    let sorted_torrents: Vec<ScrapedTorrent> = scored_torrents
        .into_iter()
        .take(100)
        .map(|(t, _)| t)
        .collect();

    info!(
        "Total unique torrents: {}, Top quality results: {}",
        total_scraped,
        sorted_torrents.len()
    );

    let streams: Vec<StremioStream> = sorted_torrents
        .into_iter()
        .map(|t| {
            let info = TorrentInfo::from_scraped_torrent(&t);
            let mut stream = StremioStream::from_torrent_info(&info, &base_url);

            stream.url = Some(format!("{}/resolve/{}/{}", base_url, t.info_hash, id));

            stream.name = format!("[RD]+\n{}", stream.name);
            stream.behavior_hints = serde_json::json!({
                "bingeGroup": format!("torrent-{}", t.source),
                "filename": t.title
            });
            stream
        })
        .collect();

    Json(StreamResponse { streams })
}

fn score_search_query(
    query: &str,
    metadata_title: &str,
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> i32 {
    let query_lower = query.to_lowercase();
    let metadata_lower = metadata_title.to_lowercase();
    let mut score = 0;

    if query_lower == metadata_lower {
        score += 50;
    } else if query_lower.starts_with(&metadata_lower) {
        score += 25;
    }

    if let Some(episode) = target_episode {
        let season = target_season.unwrap_or(1);

        if is_strong_explicit_episode_query(query, season, episode) {
            score += 52;
        } else if is_explicit_episode_candidate(query, season, episode) {
            score += 24;
        } else if matching::extract_season(query) == Some(season) {
            score -= 40;
        }

        if let Some(year) = target_year {
            if query_lower.contains(&year.to_string()) {
                score += 10;
            }
        }
    } else if let Some(year) = target_year {
        if query_lower.contains(&year.to_string()) {
            score += 20;
        }
    }

    score - query.len() as i32 / if target_episode.is_some() { 5 } else { 8 }
}

fn select_search_queries(
    mut search_queries: Vec<String>,
    metadata_title: &str,
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> Vec<String> {
    search_queries.sort_by(|a, b| {
        let a_score = score_search_query(
            a,
            metadata_title,
            target_year,
            target_season,
            target_episode,
        );
        let b_score = score_search_query(
            b,
            metadata_title,
            target_year,
            target_season,
            target_episode,
        );
        b_score.cmp(&a_score)
    });
    search_queries.dedup();

    let has_series_context = target_season.is_some() || target_episode.is_some();
    if !has_series_context {
        search_queries.truncate(8);
        return search_queries;
    }

    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for query in search_queries {
        let family_key = query_family_key(&query, has_series_context);
        if let Some((_, family_queries)) = groups.iter_mut().find(|(key, _)| *key == family_key) {
            family_queries.push(query);
        } else {
            groups.push((family_key, vec![query]));
        }
    }

    let mut selected = Vec::new();
    let mut round = 0;
    let max_queries = 8;
    while selected.len() < max_queries {
        let mut added_any = false;

        for (_, family_queries) in &groups {
            if let Some(query) = family_queries.get(round) {
                selected.push(query.clone());
                added_any = true;

                if selected.len() >= max_queries {
                    break;
                }
            }
        }

        if !added_any {
            break;
        }

        round += 1;
    }

    selected
}

fn build_match_titles(
    metadata_title: &str,
    search_queries: &[String],
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> Vec<String> {
    let has_series_context = target_season.is_some() || target_episode.is_some();
    let mut match_titles = vec![metadata_title.to_string()];

    if !has_series_context {
        return match_titles;
    }

    for query in search_queries {
        let candidate = clean_query_title_variant(query, true);
        if candidate.is_empty() || !is_reliable_match_title_alias(&candidate) {
            continue;
        }

        if !match_titles
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&candidate))
        {
            match_titles.push(candidate);
        }
    }

    match_titles
}

fn is_reliable_match_title_alias(candidate: &str) -> bool {
    if candidate.chars().any(|ch| !ch.is_ascii()) {
        return true;
    }

    let tokens: Vec<&str> = candidate
        .split_whitespace()
        .filter(|token| token.chars().any(|ch| ch.is_ascii_alphanumeric()))
        .collect();

    match tokens.as_slice() {
        [] => false,
        [token] => {
            token
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric())
                .count()
                >= 5
        }
        _ => true,
    }
}

fn best_match_score(
    match_titles: &[String],
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
    torrent: &ScrapedTorrent,
) -> i32 {
    match_titles
        .iter()
        .map(|title| {
            hydrogene::calculate_match_score(
                title,
                target_year,
                target_season,
                target_episode,
                &torrent.title,
                torrent.seeders,
                torrent.size_bytes,
            )
        })
        .max()
        .unwrap_or_default()
}

fn query_family_key(query: &str, strip_series_markers: bool) -> String {
    clean_query_title_variant(query, strip_series_markers).to_lowercase()
}

fn clean_query_title_variant(query: &str, strip_series_markers: bool) -> String {
    let mut candidate = query.replace(&['.', '_', ':'][..], " ");
    candidate = YEAR_TOKEN_REGEX.replace_all(&candidate, " ").into_owned();

    if strip_series_markers {
        candidate = SXXEXX_QUERY_REGEX.replace_all(&candidate, " ").into_owned();
        candidate = SEASON_X_EP_QUERY_REGEX
            .replace_all(&candidate, " ")
            .into_owned();
        candidate = SEASON_ONLY_QUERY_REGEX
            .replace_all(&candidate, " ")
            .into_owned();
        candidate = EPISODE_WORD_QUERY_REGEX
            .replace_all(&candidate, " ")
            .into_owned();
        candidate = TRAILING_EPISODE_QUERY_REGEX
            .replace_all(&candidate, " ")
            .into_owned();
    }

    candidate = candidate.replace('-', " ");
    MULTISPACE_REGEX
        .replace_all(candidate.trim(), " ")
        .trim()
        .to_string()
}

fn is_explicit_episode_candidate(title: &str, season: u32, episode: u32) -> bool {
    hydrogene::utils::is_exact_episode_match(title, season, episode)
        && EXPLICIT_EPISODE_HINT_REGEX.is_match(title)
}

fn is_strong_explicit_episode_query(title: &str, season: u32, episode: u32) -> bool {
    if !is_explicit_episode_candidate(title, season, episode) {
        return false;
    }

    let title_lower = title.to_lowercase();
    find_season_episode_marker(&title_lower)
        || title_lower.contains(&format!("x{:02}", episode))
        || title_lower.contains(&format!("x{}", episode))
        || title_lower.contains(&format!("e{:02}", episode))
        || title_lower.contains(&format!("e{}", episode))
        || title_lower.contains(&format!("ep{:02}", episode))
        || title_lower.contains(&format!("ep{}", episode))
        || title_lower.contains(&format!("episode {:02}", episode))
        || title_lower.contains(&format!("episode {}", episode))
        || title_lower.contains(&format!(" - {:02}", episode))
        || title_lower.contains(&format!(" - {}", episode))
}

fn quality_bucket(title: &str) -> i32 {
    let title_upper = title.to_uppercase();

    if title_upper.contains("2160P") || title_upper.contains("4K") || title_upper.contains("UHD") {
        5
    } else if title_upper.contains("1080P") {
        4
    } else if title_upper.contains("720P") {
        3
    } else if title_upper.contains("480P") {
        2
    } else {
        1
    }
}

fn is_anime_torrent(torrent: &ScrapedTorrent) -> bool {
    torrent.category.to_uppercase().contains("ANIME")
        || matches!(
            torrent.source.as_str(),
            "Nyaa" | "Nyaa/Sukebei" | "Sukebei" | "NekoBT"
        )
}

fn minimum_quality_bucket(torrent: &ScrapedTorrent, target_episode: Option<u32>) -> i32 {
    if target_episode.is_some() && is_anime_torrent(torrent) {
        1
    } else if target_episode.is_some() {
        3
    } else {
        4
    }
}

fn is_blocked_release_type(title: &str) -> bool {
    let title_upper = title.to_uppercase();

    [
        " TELESYNC",
        ".TELESYNC",
        " TS ",
        ".TS.",
        " HDTS",
        ".HDTS",
        " CAM ",
        ".CAM.",
        " HDCAM",
        ".HDCAM",
        " TELECINE",
        ".TELECINE",
    ]
    .iter()
    .any(|marker| title_upper.contains(marker))
}

fn configured_min_seeders() -> i32 {
    std::env::var("MIN_SEEDERS")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap_or(1)
}

fn passes_title_relevance_filter(
    match_titles: &[String],
    torrent_title: &str,
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> bool {
    if !match_titles
        .iter()
        .any(|title| matching::has_required_title_tokens(title, torrent_title))
    {
        return false;
    }

    if target_season.is_none() && target_episode.is_none() {
        let torrent_upper = torrent_title.to_uppercase();
        if torrent_upper.contains("COLLECTION") {
            return false;
        }

        if let Some(expected_year) = target_year {
            let years = matching::extract_all_years(torrent_title);
            if !years.is_empty() && years.iter().any(|year| *year != expected_year) {
                return false;
            }
        }
    }

    if target_season.is_some() || target_episode.is_some() {
        let torrent_upper = torrent_title.to_uppercase();
        let extra_markers = [" OAD", ".OAD", " OVA", ".OVA", " JUNIOR HIGH", " CHIMI"];
        if extra_markers
            .iter()
            .any(|marker| torrent_upper.contains(marker))
            && !match_titles
                .iter()
                .any(|title| title.to_uppercase().contains("OAD"))
            && !match_titles
                .iter()
                .any(|title| title.to_uppercase().contains("OVA"))
        {
            return false;
        }

        if let Some(expected_year) = target_year {
            let years = matching::extract_all_years(torrent_title);
            if !years.is_empty() && years.iter().all(|year| *year != expected_year) {
                return false;
            }
        }
    }

    if let Some(expected_season) = target_season {
        if let Some(found_season) = matching::extract_season(torrent_title) {
            if found_season != expected_season {
                return false;
            }
        }
    }

    true
}

fn torrent_passes_match_filters(
    torrent: &ScrapedTorrent,
    match_titles: &[String],
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
    min_seeders: i32,
) -> bool {
    torrent.seeders >= min_seeders
        && !is_blocked_release_type(&torrent.title)
        && quality_bucket(&torrent.title) >= minimum_quality_bucket(torrent, target_episode)
        && passes_quality_size_filter(torrent, target_season, target_episode)
        && passes_title_relevance_filter(
            match_titles,
            &torrent.title,
            target_year,
            target_season,
            target_episode,
        )
}

fn candidate_torrents_for_matching<'a>(
    torrents: &'a [ScrapedTorrent],
    match_titles: &[String],
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> Vec<&'a ScrapedTorrent> {
    let min_seeders = configured_min_seeders();
    torrents
        .iter()
        .filter(|torrent| {
            torrent_passes_match_filters(
                torrent,
                match_titles,
                target_year,
                target_season,
                target_episode,
                min_seeders,
            )
        })
        .collect()
}

fn is_episode_style_release(title: &str) -> bool {
    let title_lower = title.to_lowercase();

    find_season_episode_marker(&title_lower)
        || SEASON_X_EP_QUERY_REGEX.is_match(title)
        || EPISODE_WORD_QUERY_REGEX.is_match(title)
        || EXPLICIT_EPISODE_TITLE_REGEX.is_match(title)
}

fn is_probable_season_pack_result(title: &str, target_season: u32, target_episode: u32) -> bool {
    if is_explicit_episode_candidate(title, target_season, target_episode) {
        return false;
    }

    if let Some(found_season) = matching::extract_season(title) {
        if found_season != target_season {
            return false;
        }
    }

    let title_upper = title.to_uppercase();
    let pack_markers = [
        " COMPLETE",
        ".COMPLETE",
        " BATCH",
        ".BATCH",
        " PACK",
        ".PACK",
        "全集",
        " SEASON ",
        ".SEASON.",
    ];

    pack_markers
        .iter()
        .any(|marker| title_upper.contains(marker))
        || !is_episode_style_release(title)
}

fn passes_quality_size_filter(
    torrent: &ScrapedTorrent,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> bool {
    let quality = quality_bucket(&torrent.title);
    let anime_torrent = is_anime_torrent(torrent);

    match quality {
        5 => true,
        4 => {
            let min_size_mb = if target_episode.is_some() {
                if anime_torrent {
                    env_u64("MIN_1080P_ANIME_EPISODE_MB", 90)
                } else {
                    env_u64("MIN_1080P_EPISODE_MB", 350)
                }
            } else if target_season.is_some() {
                env_u64("MIN_1080P_SERIES_MB", 2000)
            } else {
                env_u64("MIN_1080P_MOVIE_MB", 3000)
            };

            torrent.size_bytes == 0 || torrent.size_bytes >= min_size_mb * 1024 * 1024
        }
        3 if target_episode.is_some() => {
            let min_size_mb = if anime_torrent {
                env_u64("MIN_720P_ANIME_EPISODE_MB", 60)
            } else {
                env_u64("MIN_720P_EPISODE_MB", 150)
            };
            torrent.size_bytes == 0 || torrent.size_bytes >= min_size_mb * 1024 * 1024
        }
        2 if target_episode.is_some() && anime_torrent => {
            let min_size_mb = env_u64("MIN_480P_ANIME_EPISODE_MB", 40);
            torrent.size_bytes == 0 || torrent.size_bytes >= min_size_mb * 1024 * 1024
        }
        1 if target_episode.is_some() && anime_torrent => torrent.size_bytes == 0,
        _ => false,
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

async fn scrape_queries_progressively(
    scraper_manager: Arc<ScraperManager>,
    content_type: &str,
    search_queries: &[String],
    match_titles: &[String],
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> Vec<ScrapedTorrent> {
    let mut all_torrents = Vec::new();
    let query_batches = build_query_batches(search_queries, target_season, target_episode);

    for (batch_index, batch) in query_batches.iter().enumerate() {
        let scraped = scrape_query_batch(scraper_manager.clone(), content_type, batch).await;
        all_torrents.extend(scraped);

        let unique = dedupe_torrents_by_hash(all_torrents.clone());

        if batch_index + 1 < query_batches.len() {
            let completed_batches = batch_index + 1;
            let can_stop_on_exact_match = target_episode.is_none() || completed_batches >= 2;

            if (can_stop_on_exact_match
                && should_stop_query_expansion(
                    &unique,
                    match_titles,
                    target_year,
                    target_season,
                    target_episode,
                ))
                || should_stop_episode_fallback(
                    &unique,
                    match_titles,
                    target_year,
                    target_season,
                    target_episode,
                    completed_batches,
                )
            {
                info!(
                    "Stopping query expansion early after batch {}/{} with {} unique torrents",
                    completed_batches,
                    query_batches.len(),
                    unique.len()
                );
                return unique;
            }
        }
    }

    dedupe_torrents_by_hash(all_torrents)
}

async fn scrape_query_batch(
    scraper_manager: Arc<ScraperManager>,
    content_type: &str,
    queries: &[String],
) -> Vec<ScrapedTorrent> {
    use futures::stream::{FuturesUnordered, StreamExt};

    let mut stream = FuturesUnordered::new();

    for query in queries {
        let manager = scraper_manager.clone();
        let query = query.clone();
        let content_type = content_type.to_string();

        stream.push(async move {
            info!("Scraping for query: {}", query);
            let scraped = manager.scrape_all(&query, &content_type).await;
            (query, scraped)
        });
    }

    let mut all_torrents = Vec::new();
    while let Some((query, scraped)) = stream.next().await {
        info!(
            "Scraper found {} results for query: {}",
            scraped.len(),
            query
        );
        all_torrents.extend(scraped);
    }

    all_torrents
}

fn build_query_batches(
    search_queries: &[String],
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> Vec<Vec<String>> {
    match search_queries.len() {
        0 => Vec::new(),
        1 => vec![search_queries.to_vec()],
        _ if target_episode.is_some() => {
            let episode = target_episode.unwrap_or_default();
            let season = target_season.unwrap_or(1);
            let mut ordered = search_queries.to_vec();

            ordered.sort_by_key(|query| {
                if is_strong_explicit_episode_query(query, season, episode) {
                    0
                } else if is_explicit_episode_candidate(query, season, episode) {
                    1
                } else if matching::extract_season(query) == Some(season) {
                    3
                } else {
                    2
                }
            });

            let mut batches = Vec::new();
            for query in ordered.iter().take(5) {
                batches.push(vec![query.clone()]);
            }

            if ordered.len() > 5 {
                batches.push(ordered[5..].to_vec());
            }

            batches
        }
        2 => vec![search_queries.to_vec()],
        3 | 4 => vec![search_queries[..2].to_vec(), search_queries[2..].to_vec()],
        _ => vec![
            search_queries[..2].to_vec(),
            search_queries[2..4].to_vec(),
            search_queries[4..].to_vec(),
        ],
    }
}

fn find_season_episode_marker(query_lower: &str) -> bool {
    let bytes = query_lower.as_bytes();

    for window in bytes.windows(6) {
        if window[0] == b's'
            && window[1].is_ascii_digit()
            && window[2].is_ascii_digit()
            && window[3] == b'e'
            && window[4].is_ascii_digit()
            && window[5].is_ascii_digit()
        {
            return true;
        }
    }

    false
}

fn dedupe_torrents_by_hash(torrents: Vec<ScrapedTorrent>) -> Vec<ScrapedTorrent> {
    let mut seen_hashes = HashSet::new();
    torrents
        .into_iter()
        .filter(|torrent| seen_hashes.insert(torrent.info_hash.clone()))
        .collect()
}

fn should_stop_query_expansion(
    torrents: &[ScrapedTorrent],
    match_titles: &[String],
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> bool {
    let candidates = candidate_torrents_for_matching(
        torrents,
        match_titles,
        target_year,
        target_season,
        target_episode,
    );

    if candidates.is_empty() {
        return false;
    }

    let mut scores: Vec<i32> = candidates
        .iter()
        .map(|torrent| {
            best_match_score(
                match_titles,
                target_year,
                target_season,
                target_episode,
                torrent,
            )
        })
        .collect();

    scores.sort_unstable_by(|a, b| b.cmp(a));

    if let (Some(target_season), Some(target_episode)) = (target_season, target_episode) {
        let exact_episode_matches = candidates
            .iter()
            .filter(|torrent| {
                is_explicit_episode_candidate(&torrent.title, target_season, target_episode)
            })
            .count();

        if exact_episode_matches == 0 {
            return false;
        }

        let top_score = scores.first().copied().unwrap_or_default();
        let strong_count = scores.iter().take(6).filter(|score| **score >= 95).count();

        if top_score >= 115
            && exact_episode_matches >= 1
            && strong_count >= 1
            && !candidates.is_empty()
        {
            return true;
        }

        if top_score >= 95
            && exact_episode_matches >= 2
            && strong_count >= 2
            && candidates.len() >= 2
        {
            return true;
        }
    }

    if candidates.len() < 12 {
        return false;
    }

    let strong_threshold = if target_episode.is_some() {
        105
    } else if target_season.is_some() {
        85
    } else {
        70
    };

    let strong_count = scores
        .iter()
        .take(12)
        .filter(|score| **score >= strong_threshold)
        .count();
    let top_score = scores.first().copied().unwrap_or_default();

    strong_count >= 6 || (strong_count >= 4 && top_score >= strong_threshold + 15)
}

fn should_stop_episode_fallback(
    torrents: &[ScrapedTorrent],
    match_titles: &[String],
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
    completed_batches: usize,
) -> bool {
    let (target_season, target_episode) = match (target_season, target_episode) {
        (Some(season), Some(episode)) if completed_batches >= 3 => (season, episode),
        _ => return false,
    };

    let candidates = candidate_torrents_for_matching(
        torrents,
        match_titles,
        target_year,
        Some(target_season),
        Some(target_episode),
    );

    if candidates.len() < 2 {
        return false;
    }

    let exact_episode_matches = candidates
        .iter()
        .filter(|torrent| {
            is_explicit_episode_candidate(&torrent.title, target_season, target_episode)
        })
        .count();
    if exact_episode_matches > 0 {
        return false;
    }

    let fallback_candidates: Vec<&ScrapedTorrent> = candidates
        .into_iter()
        .filter(|torrent| {
            is_probable_season_pack_result(&torrent.title, target_season, target_episode)
        })
        .collect();

    if fallback_candidates.len() < 2 {
        return false;
    }

    let mut scores: Vec<i32> = fallback_candidates
        .iter()
        .map(|torrent| {
            best_match_score(
                match_titles,
                target_year,
                Some(target_season),
                Some(target_episode),
                torrent,
            )
        })
        .collect();
    scores.sort_unstable_by(|a, b| b.cmp(a));

    let top_score = scores.first().copied().unwrap_or_default();
    let strong_count = scores.iter().take(6).filter(|score| **score >= 80).count();

    top_score >= 90 && strong_count >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_torrent(title: &str) -> ScrapedTorrent {
        ScrapedTorrent {
            title: title.to_string(),
            info_hash: title.to_string(),
            magnet_link: String::new(),
            size_bytes: 2 * 1024 * 1024 * 1024,
            size_gb: 2.0,
            seeders: 10,
            leechers: 0,
            source: "test".to_string(),
            category: "tv".to_string(),
        }
    }

    #[test]
    fn select_search_queries_keeps_multiple_alias_families() {
        let selected = select_search_queries(
            vec![
                "Attack on Titan 2013 01".to_string(),
                "Attack on Titan 2013 E01".to_string(),
                "Shingeki no Kyojin 01".to_string(),
                "Shingeki no Kyojin E01".to_string(),
                "進撃の巨人 01".to_string(),
            ],
            "Attack on Titan",
            Some(2013),
            Some(1),
            Some(1),
        );

        assert!(selected
            .iter()
            .any(|query| query.contains("Shingeki no Kyojin")));
        assert!(selected.iter().any(|query| query.contains("進撃の巨人")));
        assert!(selected[0].starts_with("Attack on Titan"));
        assert!(!selected[1].starts_with("Attack on Titan"));
    }

    #[test]
    fn score_search_query_prefers_exact_episode_over_season_only() {
        let exact = score_search_query(
            "Death Note 2006 S01E01",
            "Death Note",
            Some(2006),
            Some(1),
            Some(1),
        );
        let season_only =
            score_search_query("Death Note S01", "Death Note", Some(2006), Some(1), Some(1));

        assert!(exact > season_only);
    }

    #[test]
    fn score_search_query_prefers_strong_episode_marker_over_bare_number() {
        let strong = score_search_query(
            "Attack on Titan E01",
            "Attack on Titan",
            Some(2013),
            Some(1),
            Some(1),
        );
        let bare = score_search_query(
            "Attack on Titan 01",
            "Attack on Titan",
            Some(2013),
            Some(1),
            Some(1),
        );

        assert!(strong > bare);
    }

    #[test]
    fn build_match_titles_extracts_anime_aliases() {
        let match_titles = build_match_titles(
            "Attack on Titan",
            &[
                "Attack on Titan 2013 E01".to_string(),
                "Shingeki no Kyojin 01".to_string(),
                "SnK - 01".to_string(),
                "進撃の巨人 01".to_string(),
            ],
            Some(1),
            Some(1),
        );

        assert!(match_titles.iter().any(|title| title == "Attack on Titan"));
        assert!(match_titles
            .iter()
            .any(|title| title == "Shingeki no Kyojin"));
        assert!(match_titles.iter().any(|title| title == "進撃の巨人"));
        assert!(!match_titles.iter().any(|title| title == "SnK"));
    }

    #[test]
    fn title_relevance_accepts_alternate_series_titles() {
        assert!(passes_title_relevance_filter(
            &[
                "Attack on Titan".to_string(),
                "Shingeki no Kyojin".to_string()
            ],
            "[SubsPlease] Shingeki no Kyojin - 01 (1080p)",
            Some(2013),
            Some(1),
            Some(1),
        ));
    }

    #[test]
    fn title_relevance_rejects_wrong_series_year() {
        assert!(!passes_title_relevance_filter(
            &["Avatar The Last Airbender".to_string()],
            "Avatar The Last Airbender S01E01 2005 1080p",
            Some(2024),
            Some(1),
            Some(1),
        ));
    }

    #[test]
    fn early_stop_requires_exact_episode_match() {
        let torrents = vec![fake_torrent("Death Note Season 1 S01 720p BluRay x264-W4F")];

        assert!(!should_stop_query_expansion(
            &torrents,
            &["Death Note".to_string()],
            Some(2006),
            Some(1),
            Some(1),
        ));
    }

    #[test]
    fn early_stop_allows_exact_episode_match() {
        let torrents = vec![
            fake_torrent("Death Note S01E01 1080p WEB-DL"),
            fake_torrent("Death Note S01E01 720p WEB-DL"),
        ];

        assert!(should_stop_query_expansion(
            &torrents,
            &["Death Note".to_string()],
            Some(2006),
            Some(1),
            Some(1),
        ));
    }

    #[test]
    fn build_query_batches_prioritizes_explicit_episode_queries() {
        let batches = build_query_batches(
            &[
                "Attack on Titan 01".to_string(),
                "Shingeki no Kyojin 01".to_string(),
                "Attack on Titan E01".to_string(),
                "Shingeki no Kyojin E01".to_string(),
                "Attack on Titan S01".to_string(),
            ],
            Some(1),
            Some(1),
        );

        assert_eq!(batches[0], vec!["Attack on Titan E01".to_string()]);
        assert_eq!(batches[1], vec!["Shingeki no Kyojin E01".to_string()]);
        assert_eq!(batches[2], vec!["Attack on Titan 01".to_string()]);
    }

    #[test]
    fn select_search_queries_keeps_more_episode_aliases() {
        let selected = select_search_queries(
            vec![
                "Attack on Titan E01".to_string(),
                "Attack on Titan 01".to_string(),
                "Attack on Titan [01]".to_string(),
                "Shingeki no Kyojin E01".to_string(),
                "Shingeki no Kyojin 01".to_string(),
                "進撃の巨人 01".to_string(),
                "AoT E01".to_string(),
                "SnK 01".to_string(),
                "Attack on Titan - 01v2".to_string(),
            ],
            "Attack on Titan",
            Some(2013),
            Some(1),
            Some(1),
        );

        assert!(selected.len() >= 8);
    }

    #[test]
    fn title_relevance_rejects_ova_extras_for_main_series() {
        assert!(!passes_title_relevance_filter(
            &["Attack on Titan".to_string()],
            "[Yameii] Shingeki no Kyojin OAD - 01 [WEB-DL 1080p]",
            Some(2013),
            Some(1),
            Some(1),
        ));
    }

    #[test]
    fn episode_fallback_stop_allows_strong_season_pack_after_multiple_batches() {
        let torrents = vec![
            fake_torrent("Attack on Titan Season 01 S01 COMPLETE 1080p BDRip x265"),
            fake_torrent("Attack on Titan Season 01 S01 COMPLETE 720p WEB-DL"),
            fake_torrent("Attack on Titan Season 01 1080p BluRay"),
            fake_torrent("Attack on Titan 2013 Season 01 Complete 1080p"),
            fake_torrent("Attack on Titan 2013 S01 720p"),
            fake_torrent("Attack on Titan 2013 Complete Season 01 1080p"),
        ];

        assert!(should_stop_episode_fallback(
            &torrents,
            &["Attack on Titan".to_string()],
            Some(2013),
            Some(1),
            Some(1),
            3,
        ));
    }

    #[test]
    fn episode_fallback_ignores_filtered_episode_noise() {
        let torrents = vec![
            fake_torrent("[Yameii] Shingeki no Kyojin OAD - 01 [WEB-DL 1080p]"),
            fake_torrent("Attack on Titan Season 01 COMPLETE 1080p BluRay"),
            fake_torrent("Attack on Titan 2013 Season 01 720p WEB-DL"),
        ];

        assert!(should_stop_episode_fallback(
            &torrents,
            &[
                "Attack on Titan".to_string(),
                "Shingeki no Kyojin".to_string()
            ],
            Some(2013),
            Some(1),
            Some(1),
            3,
        ));
    }

    #[test]
    fn anime_episode_filters_allow_small_anime_releases() {
        let torrent = ScrapedTorrent {
            title: "[SubsPlease] One Piece - 1098 (1080p)".to_string(),
            info_hash: "anime-small".to_string(),
            magnet_link: String::new(),
            size_bytes: 120 * 1024 * 1024,
            size_gb: 0.12,
            seeders: 10,
            leechers: 0,
            source: "Nyaa".to_string(),
            category: "Anime - English-translated".to_string(),
        };

        assert!(torrent_passes_match_filters(
            &torrent,
            &["One Piece".to_string()],
            None,
            Some(1),
            Some(1098),
            1,
        ));
    }

    #[test]
    fn standard_episode_filters_stay_strict_for_non_anime_releases() {
        let torrent = ScrapedTorrent {
            title: "Breaking Bad S01E01 1080p WEB-DL".to_string(),
            info_hash: "tv-small".to_string(),
            magnet_link: String::new(),
            size_bytes: 120 * 1024 * 1024,
            size_gb: 0.12,
            seeders: 10,
            leechers: 0,
            source: "test".to_string(),
            category: "TV".to_string(),
        };

        assert!(!torrent_passes_match_filters(
            &torrent,
            &["Breaking Bad".to_string()],
            Some(2008),
            Some(1),
            Some(1),
            1,
        ));
    }
}

async fn resolve_handler(
    State(state): State<AppState>,
    Path((hash, id)): Path<(String, String)>,
) -> Result<Redirect, (axum::http::StatusCode, String)> {
    info!("Resolve request for hash: {}, id: {}", hash, id);

    let (_, season, episode) = request_targets(&id);

    if season.is_some() || episode.is_some() {
        info!("Target for pack selection: S{:?}E{:?}", season, episode);
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(300),
        state
            .debrid_client
            .resolve_magnet_with_status(&hash, season, episode),
    )
    .await
    {
        Ok(Ok(ResolveResult::Ready(video_url))) => {
            info!("Torrent {} ready, redirecting to video", hash);
            Ok(Redirect::temporary(&video_url))
        }
        Ok(Ok(ResolveResult::Downloading(progress))) => {
            info!("Torrent {} is downloading ({}%)", hash, progress);
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!("⏳ Torrent in download su Real-Debrid ({}% completato). Riprova tra 2-3 minuti.", progress),
            ))
        }
        Ok(Ok(ResolveResult::Queued)) => {
            info!("Torrent {} is queued for download", hash);
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "⏳ Torrent in coda su Real-Debrid. Riprova tra 2-3 minuti.".to_string(),
            ))
        }
        Ok(Ok(ResolveResult::Processing)) => {
            info!("Torrent {} is processing metadata", hash);
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "⏳ Torrent in elaborazione su Real-Debrid. Riprova tra 1-2 minuti.".to_string(),
            ))
        }
        Ok(Err(e)) => {
            error!("Failed to resolve {}: {}", hash, e);
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "❌ Errore: {}. Il torrent potrebbe non essere disponibile.",
                    e
                ),
            ))
        }
        Err(_) => {
            error!("Timeout for {} after 5 minutes", hash);
            Err((
                axum::http::StatusCode::REQUEST_TIMEOUT,
                "⏱️ Timeout: il torrent sta ancora scaricando su Real-Debrid. Riprova tra qualche minuto.".to_string(),
            ))
        }
    }
}
